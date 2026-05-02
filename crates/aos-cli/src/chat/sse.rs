use std::collections::VecDeque;
use std::pin::Pin;

use anyhow::{Context, Result, anyhow};
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::Value;

type ByteStream = Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>;

pub(crate) struct JournalEventStream {
    stream: ByteStream,
    decoder: SseDecoder,
    pending: VecDeque<JournalSseEvent>,
}

impl JournalEventStream {
    pub(crate) fn new(response: reqwest::Response) -> Self {
        Self {
            stream: response.bytes_stream().boxed(),
            decoder: SseDecoder::default(),
            pending: VecDeque::new(),
        }
    }

    pub(crate) async fn next_event(&mut self) -> Result<Option<JournalSseEvent>> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }
            let Some(chunk) = self.stream.next().await else {
                let events = self.decoder.finish()?;
                self.pending.extend(
                    events
                        .into_iter()
                        .map(parse_journal_sse_event)
                        .collect::<Result<Vec<_>>>()?,
                );
                if let Some(event) = self.pending.pop_front() {
                    return Ok(Some(event));
                }
                return Ok(None);
            };
            let events = self
                .decoder
                .feed(&chunk.context("read journal SSE chunk")?)?;
            self.pending.extend(
                events
                    .into_iter()
                    .map(parse_journal_sse_event)
                    .collect::<Result<Vec<_>>>()?,
            );
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JournalSseEvent {
    JournalRecord {
        seq: u64,
        kind: String,
        next_from: u64,
        record: Value,
    },
    WorldHead {
        head: u64,
        next_from: u64,
        retained_from: u64,
    },
    Gap {
        requested_from: u64,
        retained_from: u64,
        next_from: u64,
    },
    Error {
        message: String,
    },
    Unknown {
        event: String,
        data: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SseDecoder {
    buffer: Vec<u8>,
    current: SseEventBuilder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub event: String,
    pub id: Option<String>,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct SseEventBuilder {
    event: Option<String>,
    id: Option<String>,
    data: String,
}

impl SseDecoder {
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>> {
        self.buffer.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line = self.buffer.drain(..=newline).collect::<Vec<_>>();
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if let Some(event) = self.parse_line(&line)? {
                out.push(event);
            }
        }
        Ok(out)
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<SseEvent>> {
        let mut out = Vec::new();
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            if let Some(event) = self.parse_line(&line)? {
                out.push(event);
            }
        }
        if let Some(event) = self.current.dispatch() {
            out.push(event);
        }
        Ok(out)
    }

    fn parse_line(&mut self, line: &[u8]) -> Result<Option<SseEvent>> {
        if line.is_empty() {
            return Ok(self.current.dispatch());
        }
        if line.first() == Some(&b':') {
            return Ok(None);
        }
        let line = std::str::from_utf8(line).context("SSE line is not UTF-8")?;
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.current.event = Some(value.to_string()),
            "id" => self.current.id = Some(value.to_string()),
            "data" => {
                if !self.current.data.is_empty() {
                    self.current.data.push('\n');
                }
                self.current.data.push_str(value);
            }
            "retry" => {}
            _ => {}
        }
        Ok(None)
    }
}

impl SseEventBuilder {
    fn dispatch(&mut self) -> Option<SseEvent> {
        if self.event.is_none() && self.id.is_none() && self.data.is_empty() {
            return None;
        }
        let event = SseEvent {
            event: self.event.take().unwrap_or_else(|| "message".into()),
            id: self.id.take(),
            data: std::mem::take(&mut self.data),
        };
        Some(event)
    }
}

fn parse_journal_sse_event(event: SseEvent) -> Result<JournalSseEvent> {
    match event.event.as_str() {
        "journal_record" => {
            let data: JournalRecordData =
                serde_json::from_str(&event.data).context("decode journal_record SSE data")?;
            Ok(JournalSseEvent::JournalRecord {
                seq: data.seq,
                kind: data.kind,
                next_from: data.next_from,
                record: data.record,
            })
        }
        "world_head" => {
            let data: JournalHeadData =
                serde_json::from_str(&event.data).context("decode world_head SSE data")?;
            Ok(JournalSseEvent::WorldHead {
                head: data.head,
                next_from: data.next_from,
                retained_from: data.retained_from,
            })
        }
        "gap" => {
            let data: JournalGapData =
                serde_json::from_str(&event.data).context("decode gap SSE data")?;
            Ok(JournalSseEvent::Gap {
                requested_from: data.requested_from,
                retained_from: data.retained_from,
                next_from: data.next_from,
            })
        }
        "error" => {
            let data: Value = serde_json::from_str(&event.data).unwrap_or(Value::Null);
            let message = data
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or(event.data.as_str())
                .to_string();
            Ok(JournalSseEvent::Error { message })
        }
        other if other.is_empty() => Err(anyhow!("SSE event is missing an event name")),
        other => Ok(JournalSseEvent::Unknown {
            event: other.to_string(),
            data: event.data,
        }),
    }
}

#[derive(Debug, Deserialize)]
struct JournalRecordData {
    seq: u64,
    kind: String,
    next_from: u64,
    record: Value,
}

#[derive(Debug, Deserialize)]
struct JournalHeadData {
    head: u64,
    next_from: u64,
    retained_from: u64,
}

#[derive(Debug, Deserialize)]
struct JournalGapData {
    requested_from: u64,
    retained_from: u64,
    next_from: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_split_multiline_sse_event() {
        let mut decoder = SseDecoder::default();
        assert!(
            decoder
                .feed(b"event: message\nid: 4\ndata: hello")
                .unwrap()
                .is_empty()
        );
        let events = decoder.feed(b"\ndata: world\n\n").unwrap();
        assert_eq!(
            events,
            vec![SseEvent {
                event: "message".into(),
                id: Some("4".into()),
                data: "hello\nworld".into(),
            }]
        );
    }

    #[test]
    fn ignores_comments_and_dispatches_blank_line() {
        let mut decoder = SseDecoder::default();
        let events = decoder
            .feed(b": keepalive\n\nevent: error\ndata: {\"message\":\"bad\"}\n\n")
            .unwrap();
        assert_eq!(events.len(), 1);
        let parsed = parse_journal_sse_event(events[0].clone()).unwrap();
        assert_eq!(
            parsed,
            JournalSseEvent::Error {
                message: "bad".into()
            }
        );
    }
}
