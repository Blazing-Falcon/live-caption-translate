use crate::{
    error::{Error, Result},
    events::FailReason,
};
const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_EVENT_BYTES: usize = 64 * 1024;
fn error(message: impl Into<String>) -> Error {
    Error::Translation {
        reason: FailReason::Error,
        message: message.into(),
    }
}

#[derive(Default)]
pub struct SseDecoder {
    line: Vec<u8>,
    data: Vec<u8>,
}

pub enum SseEvent {
    Done,
    Data(Vec<u8>),
}

impl SseDecoder {
    pub fn byte(&mut self, byte: u8) -> Result<Option<SseEvent>> {
        if byte != b'\n' {
            if self.line.len() == MAX_LINE_BYTES {
                return Err(error("Translation SSE line exceeded its size limit"));
            }
            self.line.push(byte);
            return Ok(None);
        }
        if self.line.last() == Some(&b'\r') {
            self.line.pop();
        }
        if self.line.is_empty() {
            if self.data.is_empty() {
                return Ok(None);
            }
            let data = std::mem::take(&mut self.data);
            if data.as_slice() == b"[DONE]" {
                return Ok(Some(SseEvent::Done));
            }
            return Ok(Some(SseEvent::Data(data)));
        }
        let line = std::mem::take(&mut self.line);
        // Decoding after the complete line preserves partial multi-byte UTF-8.
        std::str::from_utf8(&line).map_err(|_| error("Invalid UTF-8 in translation SSE event"))?;
        if let Some(data) = line.strip_prefix(b"data:") {
            let data = data.strip_prefix(b" ").map_or(data, |data| data);
            let extra = usize::from(!self.data.is_empty());
            if self
                .data
                .len()
                .saturating_add(data.len())
                .saturating_add(extra)
                > MAX_EVENT_BYTES
            {
                return Err(error("Translation SSE event exceeded its size limit"));
            }
            if extra != 0 {
                self.data.push(b'\n');
            }
            self.data.extend_from_slice(data);
        }
        Ok(None)
    }
}
