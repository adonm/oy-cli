#[derive(Debug, Default)]
pub(crate) struct SseDecoder {
    buffer: String,
}

impl SseDecoder {
    pub(crate) fn push_chunk(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut events = Vec::new();
        while let Some(index) = self.buffer.find("\n\n") {
            let frame = self.buffer[..index].to_string();
            self.buffer.drain(..index + 2);
            events.extend(sse_data_lines(&frame));
        }
        events
    }

    pub(crate) fn finish(&mut self) -> Vec<String> {
        if self.buffer.trim().is_empty() {
            self.buffer.clear();
            return Vec::new();
        }
        let frame = std::mem::take(&mut self.buffer);
        sse_data_lines(&frame)
    }
}

fn sse_data_lines(frame: &str) -> Vec<String> {
    let mut items = Vec::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim_start();
        if !data.trim().is_empty() && data != "[DONE]" {
            items.push(data.to_string());
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_decoder_extracts_json_payloads_and_drops_done_markers() {
        let mut decoder = SseDecoder::default();
        let frame = "event: response.output_text.delta\r\ndata: {\"delta\":\"hi\"}\r\n\r\n: keepalive\ndata: [DONE]\n\n";

        assert_eq!(
            decoder.push_chunk(frame.as_bytes()),
            vec!["{\"delta\":\"hi\"}".to_string()]
        );
        assert!(decoder.finish().is_empty());
    }

    #[test]
    fn sse_decoder_handles_split_frames() {
        let mut decoder = SseDecoder::default();

        assert!(decoder.push_chunk(b"data: {\"a\":").is_empty());
        assert_eq!(decoder.push_chunk(b"1}\n\n"), vec!["{\"a\":1}".to_string()]);
    }
}
