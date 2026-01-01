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

use aos_air_types::AirNode;
use aos_cbor::Hash;
use aos_effects::EffectReceipt;
use aos_kernel::StateReader;
use aos_store::Store;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::adapters::timer::TimerScheduler;
use crate::error::HostError;
use crate::host::{ExternalEvent, RunMode, WorldHost};
use aos_kernel::cell_index::CellMeta;
use aos_kernel::governance::{ManifestPatch, Proposal};
use aos_kernel::journal::ApprovalDecisionRecord;
use aos_kernel::patch_doc::{PatchDocument, compile_patch_document};
use aos_kernel::shadow::ShadowSummary;
use aos_kernel::KernelError;

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
    Snapshot {
        resp: oneshot::Sender<Result<(), HostError>>,
    },
    Shutdown {
        resp: oneshot::Sender<Result<(), HostError>>,
        /// Optional sender to propagate shutdown to the control server.
        shutdown_tx: broadcast::Sender<()>,
    },
    JournalHead {
        resp: oneshot::Sender<Result<aos_kernel::ReadMeta, HostError>>,
    },
    EventSend {
        event: ExternalEvent,
        resp: oneshot::Sender<Result<(), HostError>>,
    },
    ReceiptInject {
        receipt: EffectReceipt,
        resp: oneshot::Sender<Result<(), HostError>>,
    },
    ManifestGet {
        consistency: String,
        resp: oneshot::Sender<Result<(aos_kernel::ReadMeta, Vec<u8>), HostError>>,
    },
    DefGet {
        name: String,
        resp: oneshot::Sender<Result<AirNode, HostError>>,
    },
    DefList {
        kinds: Option<Vec<String>>,
        prefix: Option<String>,
        resp: oneshot::Sender<Result<Vec<aos_kernel::DefListing>, HostError>>,
    },
    StateGet {
        reducer: String,
        key: Option<Vec<u8>>,
        consistency: String,
        resp: oneshot::Sender<Result<Option<(aos_kernel::ReadMeta, Option<Vec<u8>>)>, HostError>>,
    },
    StateList {
        reducer: String,
        resp: oneshot::Sender<Result<Vec<CellMeta>, HostError>>,
    },
    PutBlob {
        data: Vec<u8>,
        resp: oneshot::Sender<Result<String, HostError>>,
    },
    BlobGet {
        hash_hex: String,
        resp: oneshot::Sender<Result<Vec<u8>, HostError>>,
    },

    GovPropose {
        patch: GovernancePatchInput,
        description: Option<String>,
        resp: oneshot::Sender<Result<u64, HostError>>,
    },
    GovShadow {
        proposal_id: u64,
        resp: oneshot::Sender<Result<ShadowSummary, HostError>>,
    },
    GovApprove {
        proposal_id: u64,
        approver: String,
        decision: ApprovalDecisionRecord,
        resp: oneshot::Sender<Result<(), HostError>>,
    },
    GovApply {
        proposal_id: u64,
        resp: oneshot::Sender<Result<(), HostError>>,
    },
    GovList {
        resp: oneshot::Sender<Result<Vec<Proposal>, HostError>>,
    },
    GovGet {
        proposal_id: u64,
        resp: oneshot::Sender<Result<Proposal, HostError>>,
    },
}

#[derive(Debug)]
pub enum GovernancePatchInput {
    Manifest(ManifestPatch),
    PatchDoc(PatchDocument),
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
            let now_ns = self.host.kernel().logical_time_now_ns();
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
            ControlMsg::EventSend { event: evt, resp } => {
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
            ControlMsg::ReceiptInject { receipt, resp } => {
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
            ControlMsg::StateGet {
                reducer,
                key,
                consistency,
                resp,
            } => {
                let consistency = parse_consistency(&self.host, &consistency);
                let result = self
                    .host
                    .query_state(&reducer, key.as_deref(), consistency)
                    .map(|read| (read.meta, read.value));
                let _ = resp.send(Ok(result));
            }
            ControlMsg::DefGet { name, resp } => {
                let res = self.host.get_def(&name);
                let _ = resp.send(res);
            }
            ControlMsg::DefList {
                kinds,
                prefix,
                resp,
            } => {
                let res = self.host.list_defs(kinds.as_deref(), prefix.as_deref());
                let _ = resp.send(res);
            }
            ControlMsg::StateList { reducer, resp } => {
                let res = self.host.list_cells(&reducer);
                let _ = resp.send(res);
            }
            ControlMsg::BlobGet { hash_hex, resp } => {
                let res = (|| -> Result<Vec<u8>, HostError> {
                    let hash = Hash::from_hex_str(&hash_hex)
                        .map_err(|e| HostError::Store(e.to_string()))?;
                    let bytes = self
                        .host
                        .store()
                        .get_blob(hash)
                        .map_err(|e| HostError::Store(e.to_string()))?;
                    Ok(bytes)
                })();
                let _ = resp.send(res);
            }
            ControlMsg::JournalHead { resp } => {
                let meta = self.host.kernel().get_journal_head();
                let _ = resp.send(Ok(meta));
            }
            ControlMsg::PutBlob { data, resp } => {
                let res = self.host.put_blob(&data);
                let _ = resp.send(res);
            }
            ControlMsg::ManifestGet { consistency, resp } => {
                let consistency = parse_consistency(&self.host, &consistency);
                let res = self
                    .host
                    .kernel()
                    .get_manifest(consistency)
                    .map_err(HostError::from)
                    .and_then(|read| {
                        let bytes = aos_cbor::to_canonical_cbor(&read.value)
                            .map_err(|e| HostError::Manifest(format!("encode manifest: {e}")))?;
                        Ok((read.meta, bytes))
                    });
                let _ = resp.send(res);
            }
            ControlMsg::GovPropose {
                patch,
                description,
                resp,
            } => {
                tracing::info!("Governance propose via control");
                let res = match patch {
                    GovernancePatchInput::Manifest(patch) => {
                        self.host.kernel_mut().submit_proposal(patch, description)
                    }
                    GovernancePatchInput::PatchDoc(doc) => {
                        let compiled = compile_patch_document(self.host.store(), doc)
                            .map_err(HostError::from)?;
                        self.host
                            .kernel_mut()
                            .submit_proposal(compiled, description)
                    }
                };
                let _ = resp.send(res.map_err(HostError::from));
            }
            ControlMsg::GovShadow { proposal_id, resp } => {
                tracing::info!("Governance shadow via control");
                let res = self
                    .host
                    .kernel_mut()
                    .run_shadow(proposal_id, None)
                    .map_err(HostError::from);
                let _ = resp.send(res);
            }
            ControlMsg::GovApprove {
                proposal_id,
                approver,
                decision,
                resp,
            } => {
                tracing::info!("Governance approve via control");
                let res = match decision {
                    ApprovalDecisionRecord::Approve => self
                        .host
                        .kernel_mut()
                        .approve_proposal(proposal_id, approver),
                    ApprovalDecisionRecord::Reject => self
                        .host
                        .kernel_mut()
                        .reject_proposal(proposal_id, approver),
                };
                let _ = resp.send(res.map_err(HostError::from));
            }
            ControlMsg::GovApply { proposal_id, resp } => {
                tracing::info!("Governance apply via control");
                let res = self
                    .host
                    .kernel_mut()
                    .apply_proposal(proposal_id)
                    .map_err(HostError::from);
                let _ = resp.send(res);
            }
            ControlMsg::GovList { resp } => {
                tracing::info!("Governance list via control");
                let mut proposals: Vec<Proposal> = self
                    .host
                    .kernel()
                    .governance()
                    .proposals()
                    .values()
                    .cloned()
                    .collect();
                proposals.sort_by_key(|p| p.id);
                let _ = resp.send(Ok(proposals));
            }
            ControlMsg::GovGet { proposal_id, resp } => {
                tracing::info!("Governance get via control");
                let res = self
                    .host
                    .kernel()
                    .governance()
                    .proposals()
                    .get(&proposal_id)
                    .cloned()
                    .ok_or_else(|| KernelError::ProposalNotFound(proposal_id))
                    .map_err(HostError::from);
                let _ = resp.send(res);
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

fn parse_consistency<S: Store + 'static>(host: &WorldHost<S>, s: &str) -> aos_kernel::Consistency {
    match s {
        v if v.starts_with("exact:") => {
            let h = v[6..].parse().unwrap_or(host.kernel().journal_head());
            aos_kernel::Consistency::Exact(h)
        }
        v if v.starts_with("at_least:") => {
            let h = v[9..].parse().unwrap_or(host.kernel().journal_head());
            aos_kernel::Consistency::AtLeast(h)
        }
        "exact" => aos_kernel::Consistency::Exact(host.kernel().journal_head()),
        "at_least" => aos_kernel::Consistency::AtLeast(host.kernel().journal_head()),
        _ => aos_kernel::Consistency::Head,
    }
}
