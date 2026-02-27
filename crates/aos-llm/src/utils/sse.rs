//! Server-Sent Events (SSE) parser.

/// A parsed SSE event.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
    pub retry: Option<u64>,
}

impl SseEvent {
    fn is_empty(&self) -> bool {
        self.event.is_none() && self.data.is_empty() && self.id.is_none() && self.retry.is_none()
    }
}

/// Incremental SSE parser that accepts chunks and yields events.
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: String,
    current: SseEvent,
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            current: SseEvent::default(),
        }
    }

    /// Feed a text chunk and return any completed events.
    pub fn push(&mut self, chunk: &str) -> Vec<SseEvent> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();

        while let Some(pos) = self.find_line_end() {
            let line = self.buffer[..pos].to_string();
            let next = if self.buffer[pos..].starts_with("\r\n") {
                2
            } else {
                1
            };
            self.buffer.drain(..pos + next);
            let line = line.trim_end_matches(['\r', '\n']);
            self.process_line(line, &mut events);
        }

        events
    }

    fn find_line_end(&self) -> Option<usize> {
        self.buffer.find('\n')
    }

    /// Flush any remaining buffered event when the stream ends.
    pub fn finish(mut self) -> Option<SseEvent> {
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            let line = line.trim_end_matches(['\r', '\n']);
            let mut ignored = Vec::new();
            self.process_line(line, &mut ignored);
        }
        if self.current.is_empty() {
            None
        } else {
            Some(self.current)
        }
    }

    fn process_line(&mut self, line: &str, events: &mut Vec<SseEvent>) {
        if line.is_empty() {
            if !self.current.is_empty() {
                events.push(std::mem::replace(&mut self.current, SseEvent::default()));
            }
            return;
        }

        if line.starts_with(':') {
            return;
        }

        let mut parts = line.splitn(2, ':');
        let field = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("");
        let value = value.strip_prefix(' ').unwrap_or(value);

        match field {
            "event" => self.current.event = Some(value.to_string()),
            "data" => {
                if !self.current.data.is_empty() {
                    self.current.data.push('\n');
                }
                self.current.data.push_str(value);
            }
            "id" => self.current.id = Some(value.to_string()),
            "retry" => {
                if let Ok(parsed) = value.parse::<u64>() {
                    self.current.retry = Some(parsed);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiline_data() {
        let mut parser = SseParser::new();
        let events = parser.push("data: hello\ndata: world\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hello\nworld");
    }

    #[test]
    fn ignores_comments_and_handles_event() {
        let mut parser = SseParser::new();
        let events = parser.push(": ping\nevent: message\ndata: hi\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("message"));
        assert_eq!(events[0].data, "hi");
    }

    #[test]
    fn handles_retry_and_id() {
        let mut parser = SseParser::new();
        let events = parser.push("id: 42\nretry: 1500\ndata: ok\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id.as_deref(), Some("42"));
        assert_eq!(events[0].retry, Some(1500));
    }

    #[test]
    fn finish_flushes_trailing_event_without_terminal_newline() {
        let mut parser = SseParser::new();
        let events = parser.push("event: response.output_text.delta\ndata: {\"delta\":\"hi\"}");
        assert!(events.is_empty());
        let trailing = parser.finish().expect("expected trailing event");
        assert_eq!(
            trailing.event.as_deref(),
            Some("response.output_text.delta")
        );
        assert_eq!(trailing.data, "{\"delta\":\"hi\"}");
    }

    #[test]
    fn parses_openai_style_sse_event() {
        let mut parser = SseParser::new();
        let events =
            parser.push("event: response.output_text.delta\ndata: {\"delta\":\"hello\"}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].event.as_deref(),
            Some("response.output_text.delta")
        );
        assert_eq!(events[0].data, "{\"delta\":\"hello\"}");
    }

    #[test]
    fn parses_anthropic_style_sse_event() {
        let mut parser = SseParser::new();
        let events = parser.push("event: content_block_delta\ndata: {\"type\":\"text_delta\"}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("content_block_delta"));
        assert_eq!(events[0].data, "{\"type\":\"text_delta\"}");
    }

    #[test]
    fn parses_gemini_alt_sse_json_chunk() {
        let mut parser = SseParser::new();
        let events = parser
            .push("data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"x\"}]}}]}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].data,
            "{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"x\"}]}}]}"
        );
    }
}
