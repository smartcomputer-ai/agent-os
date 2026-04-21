use std::time::Duration;

use fabric_protocol::{ExecEvent, ExecEventKind, ExecId};
use futures_core::Stream;
use futures_util::{StreamExt, pin_mut};
use tokio::time::{self, Instant};

use crate::FabricClientError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecProgress {
    pub exec_id: Option<ExecId>,
    pub elapsed: Duration,
    pub stdout_delta: Vec<u8>,
    pub stderr_delta: Vec<u8>,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecTerminalStatus {
    Exited,
    Error,
    StreamEnded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecTranscript {
    pub exec_id: Option<ExecId>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    pub error_message: Option<String>,
    pub terminal_status: ExecTerminalStatus,
}

impl Default for ExecTranscript {
    fn default() -> Self {
        Self {
            exec_id: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: None,
            error_message: None,
            terminal_status: ExecTerminalStatus::StreamEnded,
        }
    }
}

#[derive(Debug, Default)]
struct ExecAccumulator {
    transcript: ExecTranscript,
    stdout_delta: Vec<u8>,
    stderr_delta: Vec<u8>,
}

pub async fn collect_exec_with_progress<S, F>(
    stream: S,
    progress_interval: Duration,
    mut on_progress: F,
) -> Result<ExecTranscript, FabricClientError>
where
    S: Stream<Item = Result<ExecEvent, FabricClientError>>,
    F: FnMut(ExecProgress),
{
    let progress_interval = if progress_interval.is_zero() {
        Duration::from_nanos(1)
    } else {
        progress_interval
    };
    let started_at = Instant::now();
    let progress_sleep = time::sleep(progress_interval);
    pin_mut!(stream);
    pin_mut!(progress_sleep);

    let mut accumulator = ExecAccumulator::default();

    loop {
        tokio::select! {
            event = stream.next() => {
                let Some(event) = event else {
                    accumulator.transcript.terminal_status = ExecTerminalStatus::StreamEnded;
                    break;
                };
                if handle_event(&mut accumulator, event?)? {
                    break;
                }
            }
            _ = &mut progress_sleep => {
                on_progress(accumulator.progress(started_at.elapsed()));
                progress_sleep.as_mut().reset(Instant::now() + progress_interval);
            }
        }
    }

    Ok(accumulator.transcript)
}

fn handle_event(
    accumulator: &mut ExecAccumulator,
    event: ExecEvent,
) -> Result<bool, FabricClientError> {
    accumulator.transcript.exec_id = Some(event.exec_id);
    match event.kind {
        ExecEventKind::Started => Ok(false),
        ExecEventKind::Stdout => {
            if let Some(data) = event.data {
                let bytes = data
                    .decode_bytes()
                    .map_err(FabricClientError::InvalidPayload)?;
                accumulator.transcript.stdout.extend_from_slice(&bytes);
                accumulator.stdout_delta.extend(bytes);
            }
            Ok(false)
        }
        ExecEventKind::Stderr => {
            if let Some(data) = event.data {
                let bytes = data
                    .decode_bytes()
                    .map_err(FabricClientError::InvalidPayload)?;
                accumulator.transcript.stderr.extend_from_slice(&bytes);
                accumulator.stderr_delta.extend(bytes);
            }
            Ok(false)
        }
        ExecEventKind::Exit => {
            accumulator.transcript.exit_code = event.exit_code;
            accumulator.transcript.terminal_status = ExecTerminalStatus::Exited;
            Ok(true)
        }
        ExecEventKind::Error => {
            accumulator.transcript.error_message = event.message.or_else(|| {
                event
                    .data
                    .as_ref()
                    .and_then(|data| data.as_text())
                    .map(str::to_owned)
            });
            accumulator.transcript.terminal_status = ExecTerminalStatus::Error;
            Ok(true)
        }
    }
}

impl ExecAccumulator {
    fn progress(&mut self, elapsed: Duration) -> ExecProgress {
        ExecProgress {
            exec_id: self.transcript.exec_id.clone(),
            elapsed,
            stdout_delta: std::mem::take(&mut self.stdout_delta),
            stderr_delta: std::mem::take(&mut self.stderr_delta),
            stdout_bytes: self.transcript.stdout.len() as u64,
            stderr_bytes: self.transcript.stderr.len() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_protocol::{ExecEvent, FabricBytes};
    use futures_util::stream;

    #[tokio::test(start_paused = true)]
    async fn fast_exec_finishes_before_first_interval_without_progress() {
        let events = timed_stream(vec![
            (Duration::from_secs(1), stdout("quick\n")),
            (Duration::from_secs(1), exit(0)),
        ]);
        let mut progress = Vec::new();

        let transcript = collect_exec_with_progress(events, Duration::from_secs(10), |event| {
            progress.push(event)
        })
        .await
        .unwrap();

        assert!(progress.is_empty());
        assert_eq!(transcript.terminal_status, ExecTerminalStatus::Exited);
        assert_eq!(transcript.exit_code, Some(0));
        assert_eq!(transcript.stdout, b"quick\n");
        assert_eq!(transcript.stderr, b"");
    }

    #[tokio::test(start_paused = true)]
    async fn progress_is_time_based_and_terminal_result_is_complete() {
        let events = timed_stream(vec![
            (Duration::from_secs(1), stdout("first\n")),
            (Duration::from_secs(11), stdout("second\n")),
            (Duration::from_secs(1), stderr("warn\n")),
            (Duration::from_secs(10), exit(7)),
        ]);
        let mut progress = Vec::new();

        let transcript = collect_exec_with_progress(events, Duration::from_secs(10), |event| {
            progress.push(event)
        })
        .await
        .unwrap();

        assert_eq!(progress.len(), 2);
        assert_eq!(progress[0].stdout_delta, b"first\n");
        assert_eq!(progress[0].stderr_delta, b"");
        assert_eq!(progress[0].stdout_bytes, "first\n".len() as u64);
        assert_eq!(progress[1].stdout_delta, b"second\n");
        assert_eq!(progress[1].stderr_delta, b"warn\n");
        assert_eq!(progress[1].stdout_bytes, "first\nsecond\n".len() as u64);
        assert_eq!(progress[1].stderr_bytes, "warn\n".len() as u64);
        assert_eq!(transcript.exit_code, Some(7));
        assert_eq!(transcript.stdout, b"first\nsecond\n");
        assert_eq!(transcript.stderr, b"warn\n");
    }

    #[tokio::test(start_paused = true)]
    async fn long_running_exec_emits_empty_progress_checkpoints() {
        let events = timed_stream(vec![
            (Duration::from_secs(1), started()),
            (Duration::from_secs(20), exit(0)),
        ]);
        let mut progress = Vec::new();

        let transcript = collect_exec_with_progress(events, Duration::from_secs(10), |event| {
            progress.push(event)
        })
        .await
        .unwrap();

        assert_eq!(progress.len(), 2);
        assert!(progress[0].stdout_delta.is_empty());
        assert!(progress[0].stderr_delta.is_empty());
        assert_eq!(progress[0].stdout_bytes, 0);
        assert_eq!(progress[0].stderr_bytes, 0);
        assert_eq!(progress[0].exec_id, Some(ExecId("exec-test".to_owned())));
        assert_eq!(transcript.terminal_status, ExecTerminalStatus::Exited);
        assert_eq!(transcript.exit_code, Some(0));
    }

    #[tokio::test(start_paused = true)]
    async fn binary_stdout_and_stderr_are_preserved() {
        let stdout_bytes = vec![0, 159, 255, b'\n'];
        let stderr_bytes = vec![255, 0, b'e'];
        let events = timed_stream(vec![
            (
                Duration::from_secs(1),
                stdout_bytes_event(stdout_bytes.clone()),
            ),
            (
                Duration::from_secs(1),
                stderr_bytes_event(stderr_bytes.clone()),
            ),
            (Duration::from_secs(1), exit(0)),
        ]);
        let mut progress = Vec::new();

        let transcript = collect_exec_with_progress(events, Duration::from_secs(10), |event| {
            progress.push(event)
        })
        .await
        .unwrap();

        assert!(progress.is_empty());
        assert_eq!(transcript.stdout, stdout_bytes);
        assert_eq!(transcript.stderr, stderr_bytes);
    }

    #[tokio::test(start_paused = true)]
    async fn error_event_stops_with_error_metadata() {
        let events = timed_stream(vec![
            (Duration::from_secs(1), stdout("partial\n")),
            (Duration::from_secs(1), error("boom")),
        ]);
        let mut progress = Vec::new();

        let transcript = collect_exec_with_progress(events, Duration::from_secs(10), |event| {
            progress.push(event)
        })
        .await
        .unwrap();

        assert!(progress.is_empty());
        assert_eq!(transcript.terminal_status, ExecTerminalStatus::Error);
        assert_eq!(transcript.error_message.as_deref(), Some("boom"));
        assert_eq!(transcript.stdout, b"partial\n");
        assert_eq!(transcript.exit_code, None);
    }

    #[tokio::test(start_paused = true)]
    async fn stream_end_without_exit_is_reported_as_stream_ended() {
        let events = timed_stream(vec![(Duration::from_secs(1), stdout("partial\n"))]);
        let mut progress = Vec::new();

        let transcript = collect_exec_with_progress(events, Duration::from_secs(10), |event| {
            progress.push(event)
        })
        .await
        .unwrap();

        assert!(progress.is_empty());
        assert_eq!(transcript.terminal_status, ExecTerminalStatus::StreamEnded);
        assert_eq!(transcript.stdout, b"partial\n");
        assert_eq!(transcript.exit_code, None);
    }

    fn timed_stream(
        events: Vec<(Duration, ExecEvent)>,
    ) -> impl Stream<Item = Result<ExecEvent, FabricClientError>> {
        stream::unfold(events.into_iter(), |mut events| async move {
            let (delay, event) = events.next()?;
            time::sleep(delay).await;
            Some((Ok(event), events))
        })
    }

    fn stdout(text: &str) -> ExecEvent {
        event(
            ExecEventKind::Stdout,
            Some(FabricBytes::Text(text.to_owned())),
            None,
            None,
        )
    }

    fn stderr(text: &str) -> ExecEvent {
        event(
            ExecEventKind::Stderr,
            Some(FabricBytes::Text(text.to_owned())),
            None,
            None,
        )
    }

    fn stdout_bytes_event(bytes: Vec<u8>) -> ExecEvent {
        event(
            ExecEventKind::Stdout,
            Some(FabricBytes::from_bytes_base64(&bytes)),
            None,
            None,
        )
    }

    fn stderr_bytes_event(bytes: Vec<u8>) -> ExecEvent {
        event(
            ExecEventKind::Stderr,
            Some(FabricBytes::from_bytes_base64(&bytes)),
            None,
            None,
        )
    }

    fn started() -> ExecEvent {
        event(ExecEventKind::Started, None, None, None)
    }

    fn exit(exit_code: i32) -> ExecEvent {
        event(ExecEventKind::Exit, None, Some(exit_code), None)
    }

    fn error(message: &str) -> ExecEvent {
        event(ExecEventKind::Error, None, None, Some(message.to_owned()))
    }

    fn event(
        kind: ExecEventKind,
        data: Option<FabricBytes>,
        exit_code: Option<i32>,
        message: Option<String>,
    ) -> ExecEvent {
        ExecEvent {
            exec_id: ExecId("exec-test".to_owned()),
            seq: 0,
            kind,
            data,
            exit_code,
            message,
        }
    }
}
