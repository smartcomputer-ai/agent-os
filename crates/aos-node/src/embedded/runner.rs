use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::thread;

use aos_runtime::timer::TimerScheduler;
use aos_runtime::{RunMode, now_wallclock_ns};
use serde::{Deserialize, Serialize};
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};
use tracing::warn;

use crate::WorldId;

use super::ingress::LocalIngressQueue;
use super::runtime::{LocalLogRuntime, LocalRuntimeError};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct LocalWorkerOutcome {
    pub submissions_drained: usize,
    pub frames_appended: usize,
}

pub struct LocalWorker {
    runtime: Arc<LocalLogRuntime>,
    ingress: Arc<LocalIngressQueue>,
    exec: Option<Runtime>,
    timers: Arc<Mutex<BTreeMap<WorldId, WorldTimerState>>>,
}

#[derive(Default)]
struct WorldTimerState {
    scheduler: TimerScheduler,
    rehydrated: bool,
}

impl LocalWorker {
    pub fn new(runtime: Arc<LocalLogRuntime>, ingress: Arc<LocalIngressQueue>) -> Self {
        let exec = RuntimeBuilder::new_current_thread()
            .enable_all()
            .build()
            .expect("build embedded local worker runtime");
        Self {
            runtime,
            ingress,
            exec: Some(exec),
            timers: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn run_once(&self) -> Result<LocalWorkerOutcome, LocalRuntimeError> {
        let queued = self.ingress.drain_all();
        let mut outcome = LocalWorkerOutcome {
            submissions_drained: queued.len(),
            frames_appended: 0,
        };
        for (_, submission) in queued {
            if self.runtime.execute_submission(submission)? {
                outcome.frames_appended = outcome.frames_appended.saturating_add(1);
            }
        }
        outcome.frames_appended = outcome
            .frames_appended
            .saturating_add(self.process_worlds_once()?);
        Ok(outcome)
    }

    pub fn pending_submissions(&self) -> usize {
        self.ingress.len()
    }

    fn process_worlds_once(&self) -> Result<usize, LocalRuntimeError> {
        let worlds = self.runtime.worker_worlds(u32::MAX)?;
        let mut timers = self
            .timers
            .lock()
            .expect("local worker timers mutex poisoned");
        timers.retain(|world_id, _| worlds.iter().any(|world| world.world_id == *world_id));

        let mut frames_appended = 0usize;
        for world in worlds {
            frames_appended = frames_appended
                .saturating_add(self.process_world_until_stable(world.world_id, &mut timers)?);
        }
        Ok(frames_appended)
    }

    fn process_world_until_stable(
        &self,
        world_id: WorldId,
        timers: &mut BTreeMap<WorldId, WorldTimerState>,
    ) -> Result<usize, LocalRuntimeError> {
        let timer_state = timers.entry(world_id).or_default();
        let mut frames_appended = 0usize;

        for _ in 0..32 {
            let now_ns = now_wallclock_ns();
            self.runtime.mutate_world_host(world_id, |host| {
                if !timer_state.rehydrated {
                    timer_state
                        .scheduler
                        .rehydrate_daemon_state(host.kernel_mut());
                    timer_state.rehydrated = true;
                }
                if host.logical_time_now_ns() < now_ns {
                    let _ = host.set_logical_time_ns(now_ns);
                }
                Ok(())
            })?;

            let exec = self
                .exec
                .as_ref()
                .expect("local worker runtime must exist while worker is running");
            let cycle = self.runtime.mutate_world_host(world_id, |host| {
                exec.block_on(host.run_cycle(RunMode::Daemon {
                    scheduler: &mut timer_state.scheduler,
                }))
            })?;

            let fired = self.runtime.mutate_world_host(world_id, |host| {
                host.fire_due_timers(&mut timer_state.scheduler)
            })?;
            if fired > 0 {
                self.runtime
                    .mutate_world_host(world_id, |host| host.drain())?;
            }

            let progressed = cycle.effects_dispatched > 0
                || cycle.receipts_applied > 0
                || cycle.initial_drain.ticks > 0
                || cycle.final_drain.ticks > 0
                || fired > 0;
            if progressed {
                frames_appended = frames_appended.saturating_add(1);
                continue;
            }
            return Ok(frames_appended);
        }

        warn!("local worker hit max cycle budget while processing world {world_id}");
        Ok(frames_appended)
    }
}

impl Drop for LocalWorker {
    fn drop(&mut self) {
        let Some(exec) = self.exec.take() else {
            return;
        };
        let _ = thread::Builder::new()
            .name("aos-local-worker-runtime-drop".into())
            .spawn(move || drop(exec));
    }
}
