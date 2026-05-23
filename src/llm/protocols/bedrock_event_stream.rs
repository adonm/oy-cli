use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::llm::schema::{MAX_LLM_EVENT_BYTES, MAX_LLM_SESSION_BYTES, ensure_byte_limit};

#[derive(Debug, Default, Clone)]
pub(crate) struct Decoder {
    buffer: Vec<u8>,
    offset: usize,
    session_bytes: usize,
}

impl Decoder {
    pub(crate) fn push_chunk(&mut self, chunk: &[u8]) -> Result<Vec<Value>> {
        ensure_byte_limit(
            "Bedrock event-stream session",
            self.session_bytes,
            chunk.len(),
            MAX_LLM_SESSION_BYTES,
        )?;
        ensure_byte_limit(
            "Bedrock event-stream chunk",
            0,
            chunk.len(),
            MAX_LLM_EVENT_BYTES,
        )?;
        self.session_bytes += chunk.len();
        self.append_chunk(chunk)?;
        let mut out = Vec::new();
        loop {
            let remaining = self.buffer.len().saturating_sub(self.offset);
            if remaining < 4 {
                break;
            }
            let total_len = u32::from_be_bytes(
                self.buffer[self.offset..self.offset + 4]
                    .try_into()
                    .expect("slice length checked"),
            ) as usize;
            if total_len < 16 {
                bail!("Failed to decode Bedrock Converse event-stream frame: frame too short");
            }
            if total_len > MAX_LLM_EVENT_BYTES {
                bail!(
                    "Failed to decode Bedrock Converse event-stream frame: frame length {total_len} exceeded {MAX_LLM_EVENT_BYTES} byte limit"
                );
            }
            if remaining < total_len {
                break;
            }
            let frame = &self.buffer[self.offset..self.offset + total_len];
            out.extend(decode_frame(frame)?);
            self.offset += total_len;
        }
        self.compact_if_consumed();
        Ok(out)
    }

    fn append_chunk(&mut self, chunk: &[u8]) -> Result<()> {
        if self.offset == 0 {
            ensure_byte_limit(
                "Bedrock event-stream pending frame",
                self.buffer.len(),
                chunk.len(),
                MAX_LLM_EVENT_BYTES,
            )?;
            self.buffer.extend_from_slice(chunk);
            return Ok(());
        }
        let remaining = self.buffer[self.offset..].to_vec();
        ensure_byte_limit(
            "Bedrock event-stream pending frame",
            remaining.len(),
            chunk.len(),
            MAX_LLM_EVENT_BYTES,
        )?;
        self.buffer.clear();
        self.buffer.extend_from_slice(&remaining);
        self.buffer.extend_from_slice(chunk);
        self.offset = 0;
        Ok(())
    }

    fn compact_if_consumed(&mut self) {
        if self.offset == self.buffer.len() {
            self.buffer.clear();
            self.offset = 0;
        }
    }
}

fn decode_frame(frame: &[u8]) -> Result<Option<Value>> {
    validate_crc(frame)?;
    let headers_len = u32::from_be_bytes(frame[4..8].try_into().expect("fixed slice")) as usize;
    let payload_start = 12 + headers_len;
    let payload_end = frame.len() - 4;
    if payload_start > payload_end {
        bail!("Failed to decode Bedrock Converse event-stream frame: invalid header length");
    }
    let headers = parse_headers(&frame[12..payload_start])?;
    if headers.message_type.as_deref() != Some("event") {
        return Ok(None);
    }
    let Some(event_type) = headers.event_type else {
        return Ok(None);
    };
    let payload = std::str::from_utf8(&frame[payload_start..payload_end])
        .context("Failed to parse Bedrock Converse event-stream payload as UTF-8")?;
    if payload.trim().is_empty() {
        return Ok(None);
    }
    let mut parsed: Value = serde_json::from_str(payload).with_context(|| {
        format!("Failed to parse Bedrock Converse event-stream payload: {payload}")
    })?;
    if let Some(object) = parsed.as_object_mut() {
        object.remove("p");
    }
    Ok(Some(serde_json::json!({event_type: parsed})))
}

fn validate_crc(frame: &[u8]) -> Result<()> {
    let prelude_crc = u32::from_be_bytes(frame[8..12].try_into().expect("fixed slice"));
    let expected_prelude = crc32fast::hash(&frame[..8]);
    if prelude_crc != expected_prelude {
        bail!("Failed to decode Bedrock Converse event-stream frame: invalid prelude CRC");
    }
    let message_crc = u32::from_be_bytes(frame[frame.len() - 4..].try_into().expect("fixed slice"));
    let expected_message = crc32fast::hash(&frame[..frame.len() - 4]);
    if message_crc != expected_message {
        bail!("Failed to decode Bedrock Converse event-stream frame: invalid message CRC");
    }
    Ok(())
}

#[derive(Debug, Default)]
struct Headers {
    message_type: Option<String>,
    event_type: Option<String>,
}

fn parse_headers(mut bytes: &[u8]) -> Result<Headers> {
    let mut headers = Headers::default();
    while !bytes.is_empty() {
        let name_len = bytes[0] as usize;
        bytes = &bytes[1..];
        if bytes.len() < name_len + 3 {
            bail!("Failed to decode Bedrock Converse event-stream frame: invalid header");
        }
        let name = std::str::from_utf8(&bytes[..name_len])
            .context("Failed to decode Bedrock Converse event-stream header name")?;
        bytes = &bytes[name_len..];
        let value_type = bytes[0];
        bytes = &bytes[1..];
        if value_type != 7 {
            bail!(
                "Failed to decode Bedrock Converse event-stream frame: unsupported header value type {value_type}"
            );
        }
        let value_len = u16::from_be_bytes(bytes[..2].try_into().expect("fixed slice")) as usize;
        bytes = &bytes[2..];
        if bytes.len() < value_len {
            bail!("Failed to decode Bedrock Converse event-stream frame: truncated header value");
        }
        let value = std::str::from_utf8(&bytes[..value_len])
            .context("Failed to decode Bedrock Converse event-stream header value")?
            .to_string();
        bytes = &bytes[value_len..];
        match name {
            ":message-type" => headers.message_type = Some(value),
            ":event-type" => headers.event_type = Some(value),
            _ => {}
        }
    }
    Ok(headers)
}

#[cfg(test)]
pub(crate) fn encode_test_event(event_type: &str, payload: &Value) -> Vec<u8> {
    let mut headers = Vec::new();
    push_string_header(&mut headers, ":message-type", "event");
    push_string_header(&mut headers, ":event-type", event_type);
    let payload = payload.to_string().into_bytes();
    let total_len = 12 + headers.len() + payload.len() + 4;
    let mut frame = Vec::new();
    frame.extend_from_slice(&(total_len as u32).to_be_bytes());
    frame.extend_from_slice(&(headers.len() as u32).to_be_bytes());
    frame.extend_from_slice(&crc32fast::hash(&frame).to_be_bytes());
    frame.extend_from_slice(&headers);
    frame.extend_from_slice(&payload);
    let crc = crc32fast::hash(&frame);
    frame.extend_from_slice(&crc.to_be_bytes());
    frame
}

#[cfg(test)]
fn push_string_header(out: &mut Vec<u8>, name: &str, value: &str) {
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    out.push(7);
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

#[cfg(test)]
#[path = "../test/protocols/bedrock_event_stream.rs"]
mod tests;
