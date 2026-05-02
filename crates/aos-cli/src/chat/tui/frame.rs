use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc};

const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Clone, Debug)]
pub(crate) struct FrameRequester {
    tx: mpsc::UnboundedSender<Instant>,
}

impl FrameRequester {
    pub(crate) fn new(draw_tx: broadcast::Sender<()>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let scheduler = FrameScheduler {
            rx,
            draw_tx,
            last_emitted_at: None,
        };
        tokio::spawn(scheduler.run());
        Self { tx }
    }

    pub(crate) fn schedule_frame(&self) {
        let _ = self.tx.send(Instant::now());
    }

    #[cfg(test)]
    pub(crate) fn test_dummy() -> Self {
        let (tx, _rx) = mpsc::unbounded_channel();
        Self { tx }
    }
}

struct FrameScheduler {
    rx: mpsc::UnboundedReceiver<Instant>,
    draw_tx: broadcast::Sender<()>,
    last_emitted_at: Option<Instant>,
}

impl FrameScheduler {
    async fn run(mut self) {
        let far_future = Duration::from_secs(60 * 60 * 24 * 365);
        let mut next_deadline: Option<Instant> = None;

        loop {
            let target = next_deadline.unwrap_or_else(|| Instant::now() + far_future);
            let sleep = tokio::time::sleep_until(target.into());
            tokio::pin!(sleep);

            tokio::select! {
                requested = self.rx.recv() => {
                    let Some(requested) = requested else {
                        break;
                    };
                    let requested = self.clamp_deadline(requested);
                    next_deadline = Some(next_deadline.map_or(requested, |current| current.min(requested)));
                }
                _ = &mut sleep => {
                    if next_deadline.is_some() {
                        next_deadline = None;
                        self.last_emitted_at = Some(target);
                        let _ = self.draw_tx.send(());
                    }
                }
            }
        }
    }

    fn clamp_deadline(&self, requested: Instant) -> Instant {
        self.last_emitted_at
            .and_then(|last| last.checked_add(MIN_FRAME_INTERVAL))
            .map_or(requested, |min| requested.max(min))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn schedule_frame_coalesces_multiple_requests() {
        let (draw_tx, mut draw_rx) = broadcast::channel(8);
        let requester = FrameRequester::new(draw_tx);

        requester.schedule_frame();
        requester.schedule_frame();

        let first = tokio::time::timeout(Duration::from_secs(1), draw_rx.recv()).await;
        assert!(first.is_ok());
        let second = tokio::time::timeout(Duration::from_millis(50), draw_rx.recv()).await;
        assert!(second.is_err());
    }
}
