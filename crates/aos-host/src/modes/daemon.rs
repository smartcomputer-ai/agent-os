//! Daemon mode for long-lived world execution with real timers.
//!
//! The daemon runs a select loop that:
//! 1. Fires due timers
//! 2. Processes control messages
//! 3. Handles graceful shutdown
//!
//! Timer intents are partitioned out during `run_cycle(RunMode::Daemon)` and
//! scheduled on the `TimerScheduler`. The daemon fires them via `fire_due_timers`
//! when their deadlines arrive.

use std::time::Duration;

use aos_effects::EffectReceipt;
use aos_store::Store;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::adapters::timer::TimerScheduler;
use crate::error::HostError;
use crate::host::{ExternalEvent, RunMode, WorldHost, now_wallclock_ns};

/// Convert a `std::time::Instant` to a `tokio::time::Instant`.
///
/// Tokio's instant is based on a different clock, so we compute the duration
/// from std's now and add it to tokio's now.
fn to_tokio_instant(i: std::time::Instant) -> tokio::time::Instant {
    let now = std::time::Instant::now();
    if i <= now {
        tokio::time::Instant::now()
    } else {
        tokio::time::Instant::now() + (i - now)
    }
}

/// Control message for the daemon.
///
/// These are fed into the daemon via the control channel. In P3, a `ControlServer`
/// will handle the Unix socket/stdio interface and translate JSON commands into
/// these messages.
#[derive(Debug)]
pub enum ControlMsg {
    SendEvent {
        event: ExternalEvent,
        resp: oneshot::Sender<Result<(), HostError>>,
    },
    InjectReceipt {
        receipt: EffectReceipt,
        resp: oneshot::Sender<Result<(), HostError>>,
    },
    Snapshot {
        resp: oneshot::Sender<Result<(), HostError>>,
    },
    Step {
        resp: oneshot::Sender<Result<(), HostError>>,
    },
    QueryState {
        reducer: String,
        key: Option<Vec<u8>>,
        resp: oneshot::Sender<Result<Option<Vec<u8>>, HostError>>,
    },
    JournalHead {
        resp: oneshot::Sender<Result<u64, HostError>>,
    },
    Shutdown {
        resp: oneshot::Sender<Result<(), HostError>>,
        /// Optional sender to propagate shutdown to the control server.
        shutdown_tx: broadcast::Sender<()>,
    },
}

/// World daemon for long-lived execution with real timers.
///
/// The daemon owns:
/// - A `WorldHost` for kernel + adapter interaction
/// - A `TimerScheduler` for real-time timer delivery
/// - A control channel for external commands
/// - A shutdown channel for graceful termination
pub struct WorldDaemon<S: Store + 'static> {
    host: WorldHost<S>,
    timer_scheduler: TimerScheduler,
    control_rx: mpsc::Receiver<ControlMsg>,
    shutdown_rx: broadcast::Receiver<()>,
    control_server: Option<JoinHandle<()>>,
}

impl<S: Store + 'static> WorldDaemon<S> {
    /// Create a new daemon.
    ///
    /// The caller should:
    /// 1. Create the `WorldHost`
    /// 2. Create control and shutdown channels
    /// 3. Optionally call `rehydrate_timers()` before `run()` if restoring from a snapshot
    pub fn new(
        host: WorldHost<S>,
        control_rx: mpsc::Receiver<ControlMsg>,
        shutdown_rx: broadcast::Receiver<()>,
        control_server: Option<JoinHandle<()>>,
    ) -> Self {
        let mut daemon = Self {
            host,
            timer_scheduler: TimerScheduler::new(),
            control_rx,
            shutdown_rx,
            control_server,
        };

        // Automatically rehydrate timers from pending reducer receipts so callers
        // can't forget to restore timers after a restart.
        daemon.rehydrate_timers();
        daemon
    }

    /// Rehydrate pending timers from kernel snapshot.
    ///
    /// Call this after construction but before `run()` to restore any timers
    /// that were pending when the daemon last shut down.
    pub fn rehydrate_timers(&mut self) {
        if !self.timer_scheduler.is_empty() {
            tracing::debug!("Timer scheduler already populated; skipping rehydrate");
            return;
        }
        let pending = self.host.kernel().pending_reducer_receipts_snapshot();
        self.timer_scheduler.rehydrate_from_pending(&pending);
        let count = self.timer_scheduler.len();
        if count > 0 {
            tracing::info!("Rehydrated {} pending timer(s)", count);
        }
    }

    /// Run the daemon's main loop.
    ///
    /// This loop:
    /// 1. Calculates the next timer deadline
    /// 2. Uses `tokio::select!` to wait for timer, control message, or shutdown
    /// 3. On timer: fires due timers and runs a cycle
    /// 4. On control: applies the command and runs a cycle
    /// 5. On shutdown: creates a snapshot and exits
    pub async fn run(&mut self) -> Result<(), HostError> {
        tracing::info!("World daemon started");

        // Initial drain in case there's work from previous session
        self.host.drain()?;

        // Run an initial cycle to process any startup events
        self.run_daemon_cycle().await?;

        // Track whether control channel is still open
        let mut control_open = true;

        loop {
            // Calculate next wake time
            let now_ns = now_wallclock_ns();
            let next_deadline = self.timer_scheduler.next_deadline(now_ns);

            // If control channel is closed and no timers pending, exit
            if !control_open && next_deadline.is_none() {
                tracing::info!("No pending timers and control channel closed, exiting");
                break;
            }

            let sleep_future = match next_deadline {
                Some(deadline) => tokio::time::sleep_until(to_tokio_instant(deadline)),
                None => {
                    // No timers scheduled; use a long idle timeout
                    tokio::time::sleep(Duration::from_secs(60))
                }
            };

            tokio::select! {
                // Timer fired (or idle timeout)
                _ = sleep_future => {
                    let fired = self.host.fire_due_timers(&mut self.timer_scheduler)?;
                    if fired > 0 {
                        tracing::info!("Fired {} timer(s)", fired);
                        // Run a cycle to process any effects from timer handlers
                        self.run_daemon_cycle().await?;
                    }
                }

                // Control message
                msg = self.control_rx.recv(), if control_open => {
                    match msg {
                        Some(cmd) => {
                            let should_stop = matches!(cmd, ControlMsg::Shutdown { .. });
                            self.apply_control(cmd).await?;
                            if should_stop {
                                tracing::info!("Shutdown requested via control channel");
                                break;
                            }
                        }
                        None => {
                            tracing::debug!("Control channel closed");
                            control_open = false;
                            // Don't break - continue if there are pending timers
                        }
                    }
                }

                // Shutdown signal
                _ = self.shutdown_rx.recv() => {
                    tracing::info!("Shutdown signal received");
                    break;
                }
            }
        }

        // Clean shutdown: create snapshot
        self.host.snapshot()?;
        tracing::info!("World daemon stopped");
        // Ensure control server task is joined if present
        if let Some(handle) = self.control_server.take() {
            let _ = handle.await;
        }
        Ok(())
    }

    /// Run a cycle in daemon mode.
    async fn run_daemon_cycle(&mut self) -> Result<(), HostError> {
        let outcome = self
            .host
            .run_cycle(RunMode::Daemon {
                scheduler: &mut self.timer_scheduler,
            })
            .await?;

        if outcome.effects_dispatched > 0 || outcome.receipts_applied > 0 {
            tracing::debug!(
                "Cycle: {} effects, {} receipts",
                outcome.effects_dispatched,
                outcome.receipts_applied
            );
        }
        Ok(())
    }

    /// Apply a control command.
    async fn apply_control(&mut self, cmd: ControlMsg) -> Result<(), HostError> {
        match cmd {
            ControlMsg::SendEvent { event: evt, resp } => {
                tracing::debug!("Received external event");
                let res = (|| -> Result<(), HostError> {
                    self.host.enqueue_external(evt)?;
                    Ok(())
                })();
                let res = match res {
                    Ok(_) => self.run_daemon_cycle().await.map(|_| ()),
                    Err(e) => Err(e),
                };
                let _ = resp.send(res);
            }
            ControlMsg::InjectReceipt { receipt, resp } => {
                tracing::debug!("Injecting receipt");
                let res = (|| -> Result<(), HostError> {
                    self.host.kernel_mut().handle_receipt(receipt)?;
                    Ok(())
                })();
                let res = match res {
                    Ok(_) => self.run_daemon_cycle().await.map(|_| ()),
                    Err(e) => Err(e),
                };
                let _ = resp.send(res);
            }
            ControlMsg::Snapshot { resp } => {
                tracing::info!("Creating snapshot (by request)");
                let res = self.host.snapshot();
                let _ = resp.send(res);
            }
            ControlMsg::Step { resp } => {
                tracing::debug!("Running step (by request)");
                let res = self.run_daemon_cycle().await;
                let _ = resp.send(res.map(|_| ()));
            }
            ControlMsg::QueryState { reducer, key, resp } => {
                let result = self.host.state(&reducer, key.as_deref()).cloned();
                let _ = resp.send(Ok(result));
            }
            ControlMsg::JournalHead { resp } => {
                let heights = self.host.heights();
                let _ = resp.send(Ok(heights.head));
            }
            ControlMsg::Shutdown { resp, shutdown_tx } => {
                let _ = shutdown_tx.send(()); // notify control server listener
                let _ = resp.send(Ok(()));
                tracing::info!("Shutdown requested via control channel");
                // run loop will break after this handler returns
            }
        }
        Ok(())
    }

    /// Access the underlying host.
    pub fn host(&self) -> &WorldHost<S> {
        &self.host
    }

    /// Mutably access the underlying host.
    pub fn host_mut(&mut self) -> &mut WorldHost<S> {
        &mut self.host
    }

    /// Access the timer scheduler.
    pub fn timer_scheduler(&self) -> &TimerScheduler {
        &self.timer_scheduler
    }
}
