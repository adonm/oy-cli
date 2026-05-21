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
#[path = "../test/framing.rs"]
mod tests;
