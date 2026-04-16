use std::time::Instant;

use crate::config::HostedWorkerConfig;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::core::SchedulerMsg;
use super::layers::{CheckpointPolicy, IngressBridge, JournalCoordinator, WorkerScheduler};
use super::runtime::HostedWorkerRuntime;
use super::types::{SupervisorRunProfile, WorkerError};

#[derive(Clone)]
pub struct HostedWorker {
    pub config: HostedWorkerConfig,
}

impl HostedWorker {
    pub fn new(config: HostedWorkerConfig) -> Self {
        Self { config }
    }

    pub fn with_worker_runtime(&self, runtime: HostedWorkerRuntime) -> WorkerSupervisor {
        WorkerSupervisor {
            runtime,
            config: self.config.clone(),
        }
    }
}

pub struct WorkerSupervisor {
    runtime: HostedWorkerRuntime,
    config: HostedWorkerConfig,
}

pub struct WorkerSupervisorHandle {
    runtime: HostedWorkerRuntime,
    join: Option<JoinHandle<Result<(), WorkerError>>>,
    profile_rx: Option<mpsc::UnboundedReceiver<SupervisorRunProfile>>,
}

impl WorkerSupervisor {
    pub async fn serve_forever(&mut self) -> Result<(), WorkerError> {
        let (scheduler_tx, scheduler_rx) = self.prepare_scheduler()?;
        self.serve_forever_with_scheduler(scheduler_tx, scheduler_rx, None)
            .await
    }

    fn prepare_scheduler(
        &self,
    ) -> Result<
        (
            mpsc::UnboundedSender<SchedulerMsg>,
            mpsc::UnboundedReceiver<SchedulerMsg>,
        ),
        WorkerError,
    > {
        let (scheduler_tx, scheduler_rx) = tokio::sync::mpsc::unbounded_channel();
        self.runtime.set_scheduler_tx(scheduler_tx.clone())?;
        Ok((scheduler_tx, scheduler_rx))
    }

    async fn serve_forever_with_scheduler(
        &mut self,
        scheduler_tx: mpsc::UnboundedSender<SchedulerMsg>,
        mut scheduler_rx: mpsc::UnboundedReceiver<SchedulerMsg>,
        profile_tx: Option<mpsc::UnboundedSender<SupervisorRunProfile>>,
    ) -> Result<(), WorkerError> {
        self.runtime.set_max_local_continuation_slices_per_flush(
            self.config.max_local_continuation_slices_per_flush,
        )?;
        self.runtime
            .set_projection_commit_mode(self.config.projection_commit_mode)?;
        self.runtime
            .set_max_uncommitted_slices_per_world(self.config.max_uncommitted_slices_per_world)?;
        let checkpoint_policy = CheckpointPolicy::from(&self.config);
        let journal = JournalCoordinator::new(checkpoint_policy);

        let mut ingress_task = IngressBridge::spawn_polling_task(
            self.runtime.clone(),
            scheduler_tx.clone(),
            profile_tx.clone(),
        );

        if let Some(effect_rx) = self.runtime.take_effect_event_rx()? {
            WorkerScheduler::spawn_effect_forwarder(effect_rx, scheduler_tx.clone());
        }

        let flush_tx = scheduler_tx.clone();
        let flush_period = self.runtime.flush_max_delay()?;
        let flush_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(flush_period);
            loop {
                interval.tick().await;
                if flush_tx.send(SchedulerMsg::FlushTick).is_err() {
                    break;
                }
            }
        });

        let checkpoint_tx = scheduler_tx.clone();
        let checkpoint_period = self.config.checkpoint_interval;
        let checkpoint_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(checkpoint_period);
            loop {
                interval.tick().await;
                if checkpoint_tx.send(SchedulerMsg::CheckpointTick).is_err() {
                    break;
                }
            }
        });

        let result = loop {
            tokio::select! {
                ingress = &mut ingress_task => {
                    break match ingress {
                        Ok(result) => result,
                        Err(err) => Err(WorkerError::BackgroundBuild(format!("hosted ingress bridge join failed: {err}"))),
                    };
                }
                maybe_msg = scheduler_rx.recv() => {
                    let Some(msg) = maybe_msg else {
                        break Ok(());
                    };
                    let mut force_flush =
                        matches!(msg, SchedulerMsg::CheckpointTick | SchedulerMsg::Shutdown);
                    let mut checkpoint_tick = matches!(msg, SchedulerMsg::CheckpointTick);
                    let mut shutdown_requested = matches!(msg, SchedulerMsg::Shutdown);
                    let mut pending = if shutdown_requested {
                        Vec::new()
                    } else {
                        vec![msg]
                    };
                    while let Ok(queued) = scheduler_rx.try_recv() {
                        match queued {
                            SchedulerMsg::Shutdown => {
                                shutdown_requested = true;
                                force_flush = true;
                                break;
                            }
                            SchedulerMsg::FlushTick => {
                                pending.push(SchedulerMsg::FlushTick);
                            }
                            SchedulerMsg::CheckpointTick => {
                                force_flush = true;
                                checkpoint_tick = true;
                                pending.push(SchedulerMsg::CheckpointTick);
                            }
                            other => pending.push(other),
                        }
                    }
                    let mut profile = SupervisorRunProfile::default();
                    let started = Instant::now();
                    let mut core = self.runtime.lock_core()?;
                    let _ = WorkerScheduler::handle_messages(&mut core, pending)?;
                    let _ = WorkerScheduler::drive_until_quiescent(
                        &mut core,
                        force_flush,
                        &mut profile,
                    )?;
                    if checkpoint_tick {
                        let _ = journal.publish_due_checkpoints(&mut core, &mut profile)?;
                        let _ = WorkerScheduler::drive_until_quiescent(
                            &mut core,
                            true,
                            &mut profile,
                        )?;
                    }
                    profile.total += started.elapsed();
                    if let Some(profile_tx) = profile_tx.as_ref()
                        && profile.has_activity()
                    {
                        let _ = profile_tx.send(profile);
                    }
                    if shutdown_requested {
                        break Ok(());
                    }
                }
            }
        };

        ingress_task.abort();
        flush_task.abort();
        checkpoint_task.abort();
        self.runtime.clear_scheduler_tx()?;
        result
    }

    pub fn spawn(self) -> Result<WorkerSupervisorHandle, WorkerError> {
        let (scheduler_tx, scheduler_rx) = self.prepare_scheduler()?;
        Ok(WorkerSupervisorHandle::spawn(
            self,
            scheduler_tx,
            scheduler_rx,
            false,
        ))
    }

    pub fn spawn_profiled(self) -> Result<WorkerSupervisorHandle, WorkerError> {
        let (scheduler_tx, scheduler_rx) = self.prepare_scheduler()?;
        Ok(WorkerSupervisorHandle::spawn(
            self,
            scheduler_tx,
            scheduler_rx,
            true,
        ))
    }
}

impl WorkerSupervisorHandle {
    fn spawn(
        mut supervisor: WorkerSupervisor,
        scheduler_tx: mpsc::UnboundedSender<SchedulerMsg>,
        scheduler_rx: mpsc::UnboundedReceiver<SchedulerMsg>,
        capture_profiles: bool,
    ) -> Self {
        let runtime = supervisor.runtime.clone();
        let (profile_tx, profile_rx) = if capture_profiles {
            let (tx, rx) = mpsc::unbounded_channel();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        let join = tokio::spawn(async move {
            supervisor
                .serve_forever_with_scheduler(scheduler_tx, scheduler_rx, profile_tx)
                .await
        });
        Self {
            runtime,
            join: Some(join),
            profile_rx,
        }
    }

    async fn ensure_running(&mut self) -> Result<(), WorkerError> {
        let Some(join) = self.join.as_ref() else {
            return Ok(());
        };
        if !join.is_finished() {
            return Ok(());
        }
        let join = self.join.take().expect("join handle present when finished");
        match join.await {
            Ok(Ok(())) => Err(WorkerError::BackgroundBuild(
                "hosted worker exited before shutdown was requested".to_owned(),
            )),
            Ok(Err(err)) => Err(err),
            Err(err) => Err(WorkerError::BackgroundBuild(format!(
                "hosted worker join failed: {err}"
            ))),
        }
    }

    pub fn drain_profiles(&mut self) -> SupervisorRunProfile {
        let mut aggregate = SupervisorRunProfile::default();
        let Some(profile_rx) = self.profile_rx.as_mut() else {
            return aggregate;
        };
        while let Ok(profile) = profile_rx.try_recv() {
            aggregate.merge(profile);
        }
        aggregate
    }

    pub async fn wait_for_progress(
        &mut self,
        wait: std::time::Duration,
    ) -> Result<(), WorkerError> {
        let _ = self.drain_profiles();
        tokio::task::yield_now().await;
        self.ensure_running().await?;
        tokio::time::sleep(wait).await;
        self.ensure_running().await
    }

    pub async fn observe_interval(
        &mut self,
        wait: std::time::Duration,
    ) -> Result<SupervisorRunProfile, WorkerError> {
        let mut aggregate = self.drain_profiles();
        tokio::task::yield_now().await;
        self.ensure_running().await?;
        tokio::time::sleep(wait).await;
        self.ensure_running().await?;
        aggregate.merge(self.drain_profiles());
        Ok(aggregate)
    }

    pub async fn shutdown(mut self) -> Result<(), WorkerError> {
        let _ = self.runtime.request_shutdown()?;
        let Some(join) = self.join.take() else {
            return Ok(());
        };
        match join.await {
            Ok(result) => result,
            Err(err) => Err(WorkerError::BackgroundBuild(format!(
                "hosted worker join failed: {err}"
            ))),
        }
    }
}

impl Drop for WorkerSupervisorHandle {
    fn drop(&mut self) {
        let _ = self.runtime.request_shutdown();
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}
