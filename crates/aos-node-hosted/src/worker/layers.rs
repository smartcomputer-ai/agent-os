use std::time::{Duration, Instant};

use crate::config::HostedWorkerConfig;

use aos_node::EffectRuntimeEvent;

use super::core::{LocalInputMsg, SchedulerMsg};
use super::runtime::HostedWorkerRuntime;
use super::types::{HostedWorkerCore, SupervisorRunProfile, WorkerError};

#[derive(Debug, Clone, Copy)]
pub(super) struct CheckpointPolicy {
    pub interval: Duration,
    pub every_events: Option<u32>,
}

impl From<&HostedWorkerConfig> for CheckpointPolicy {
    fn from(config: &HostedWorkerConfig) -> Self {
        Self {
            interval: config.checkpoint_interval,
            every_events: config.checkpoint_every_events,
        }
    }
}

pub(super) struct IngressBridge;

impl IngressBridge {
    pub fn collect_messages(
        runtime: &HostedWorkerRuntime,
        profile: &mut SupervisorRunProfile,
    ) -> Result<Vec<SchedulerMsg>, WorkerError> {
        let started = Instant::now();
        let messages = runtime.collect_ingress_bridge_messages()?;
        let elapsed = started.elapsed();
        profile.sync_assignments += elapsed;
        if messages
            .iter()
            .any(|msg| matches!(msg, SchedulerMsg::Assignment(_)))
        {
            profile.sync_active_worlds += elapsed;
        }
        Ok(messages)
    }

    pub fn spawn_polling_task(
        runtime: HostedWorkerRuntime,
        ingress_tx: tokio::sync::mpsc::UnboundedSender<SchedulerMsg>,
        profile_tx: Option<tokio::sync::mpsc::UnboundedSender<SupervisorRunProfile>>,
    ) -> tokio::task::JoinHandle<Result<(), WorkerError>> {
        if runtime.embedded_ingress_notify().is_some() {
            return tokio::spawn(async move {
                ingress_tx.closed().await;
                Ok(())
            });
        }

        tokio::task::spawn_blocking(move || -> Result<(), WorkerError> {
            loop {
                if ingress_tx.is_closed() {
                    return Ok(());
                }

                let mut profile = SupervisorRunProfile::default();
                let messages = Self::collect_messages(&runtime, &mut profile)?;
                if let Some(profile_tx) = profile_tx.as_ref()
                    && !messages.is_empty()
                {
                    let _ = profile_tx.send(profile);
                }
                for message in messages {
                    if ingress_tx.send(message).is_err() {
                        return Ok(());
                    }
                }
            }
        })
    }
}

pub(super) struct WorkerScheduler;

impl WorkerScheduler {
    pub fn handle_messages(
        core: &mut HostedWorkerCore,
        messages: impl IntoIterator<Item = SchedulerMsg>,
    ) -> Result<bool, WorkerError> {
        let mut progressed = false;
        for message in messages {
            progressed |= core.handle_scheduler_msg(message)?;
        }
        Ok(progressed)
    }

    pub fn drive_until_quiescent(
        core: &mut HostedWorkerCore,
        force_flush: bool,
        profile: &mut SupervisorRunProfile,
    ) -> Result<(), WorkerError> {
        let _ = core.drive_scheduler_until_quiescent(force_flush, profile)?;
        Ok(())
    }

    pub fn spawn_effect_forwarder(
        mut effect_rx: tokio::sync::mpsc::Receiver<EffectRuntimeEvent<aos_node::WorldId>>,
        scheduler_tx: tokio::sync::mpsc::UnboundedSender<SchedulerMsg>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while let Some(event) = effect_rx.recv().await {
                let msg = match event {
                    EffectRuntimeEvent::WorldInput { world_id, input } => {
                        SchedulerMsg::LocalInput(LocalInputMsg { world_id, input })
                    }
                };
                if scheduler_tx.send(msg).is_err() {
                    break;
                }
            }
        })
    }
}

pub(super) struct JournalCoordinator {
    checkpoint_policy: CheckpointPolicy,
}

impl JournalCoordinator {
    pub fn new(checkpoint_policy: CheckpointPolicy) -> Self {
        Self { checkpoint_policy }
    }

    pub fn publish_due_checkpoints(
        &self,
        core: &mut HostedWorkerCore,
        profile: &mut SupervisorRunProfile,
    ) -> Result<usize, WorkerError> {
        let started = Instant::now();
        let published = core.publish_due_checkpoints(
            self.checkpoint_policy.interval,
            self.checkpoint_policy.every_events,
        )?;
        profile.publish_checkpoints += started.elapsed();
        Ok(published)
    }
}
