use anyhow::{Result, bail};

use crate::llm::schema::{MAX_LLM_EVENT_BYTES, ensure_byte_limit};

#[derive(Debug, Default)]
pub(crate) struct SseDecoder {
    buffer: String,
}

impl SseDecoder {
    pub(crate) fn push_chunk(&mut self, chunk: &[u8]) -> Result<Vec<String>> {
        ensure_byte_limit(
            "SSE event frame",
            self.buffer.len(),
            chunk.len(),
            MAX_LLM_EVENT_BYTES,
        )?;
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut events = Vec::new();
        while let Some(index) = self.buffer.find("\n\n") {
            if index > MAX_LLM_EVENT_BYTES {
                bail!("LLM provider SSE event frame exceeded {MAX_LLM_EVENT_BYTES} byte limit");
            }
            let frame = self.buffer[..index].to_string();
            self.buffer.drain(..index + 2);
            events.extend(sse_data_lines(&frame)?);
        }
        Ok(events)
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<String>> {
        if self.buffer.trim().is_empty() {
            self.buffer.clear();
            return Ok(Vec::new());
        }
        if self.buffer.len() > MAX_LLM_EVENT_BYTES {
            bail!("LLM provider SSE event frame exceeded {MAX_LLM_EVENT_BYTES} byte limit");
        }
        let frame = std::mem::take(&mut self.buffer);
        sse_data_lines(&frame)
    }
}

fn sse_data_lines(frame: &str) -> Result<Vec<String>> {
    let mut items = Vec::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim_start();
        if !data.trim().is_empty() && data != "[DONE]" {
            ensure_byte_limit("SSE event data", 0, data.len(), MAX_LLM_EVENT_BYTES)?;
            items.push(data.to_string());
        }
    }
    Ok(items)
}

#[cfg(test)]
#[path = "../test/framing.rs"]
mod tests;
