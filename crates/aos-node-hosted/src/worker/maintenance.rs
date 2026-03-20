use std::collections::BTreeSet;
use std::ops::Bound::{Excluded, Unbounded};
use std::time::Duration;

use aos_fdb::{HostedRuntimeStore, UniverseAdminStatus, UniverseId, UniverseStore};

use crate::config;

use super::WorkerError;

#[derive(Debug, Clone, Copy, Default)]
struct TaskState {
    next_run_ns: u64,
    after_universe: Option<UniverseId>,
}

#[derive(Debug, Default)]
pub struct MaintenanceScheduler {
    effect_claim_requeue: TaskState,
    timer_claim_requeue: TaskState,
    effect_dedupe_gc: TaskState,
    timer_dedupe_gc: TaskState,
    portal_dedupe_gc: TaskState,
}

impl MaintenanceScheduler {
    pub fn run_due<P>(
        &mut self,
        runtime: &P,
        config: &config::FdbWorkerConfig,
        now_ns: u64,
    ) -> Result<(), WorkerError>
    where
        P: HostedRuntimeStore + UniverseStore + 'static,
    {
        run_task(
            &mut self.effect_claim_requeue,
            runtime,
            config,
            now_ns,
            config.effect_claim_requeue_interval,
            |runtime, universe, config, now_ns| {
                let _ = runtime.requeue_expired_effect_claims(
                    universe,
                    now_ns,
                    config.ready_scan_limit,
                )?;
                Ok(())
            },
        )?;
        run_task(
            &mut self.timer_claim_requeue,
            runtime,
            config,
            now_ns,
            config.timer_claim_requeue_interval,
            |runtime, universe, config, now_ns| {
                let _ = runtime.requeue_expired_timer_claims(
                    universe,
                    now_ns,
                    config.ready_scan_limit,
                )?;
                Ok(())
            },
        )?;
        run_task(
            &mut self.effect_dedupe_gc,
            runtime,
            config,
            now_ns,
            config.effect_dedupe_gc_interval,
            |runtime, universe, config, now_ns| {
                let _ = runtime.sweep_effect_dedupe_gc(
                    universe,
                    now_ns,
                    config.dedupe_gc_sweep_limit,
                )?;
                Ok(())
            },
        )?;
        run_task(
            &mut self.timer_dedupe_gc,
            runtime,
            config,
            now_ns,
            config.timer_dedupe_gc_interval,
            |runtime, universe, config, now_ns| {
                let _ = runtime.sweep_timer_dedupe_gc(
                    universe,
                    now_ns,
                    config.dedupe_gc_sweep_limit,
                )?;
                Ok(())
            },
        )?;
        run_task(
            &mut self.portal_dedupe_gc,
            runtime,
            config,
            now_ns,
            config.portal_dedupe_gc_interval,
            |runtime, universe, config, now_ns| {
                let _ = runtime.sweep_portal_dedupe_gc(
                    universe,
                    now_ns,
                    config.dedupe_gc_sweep_limit,
                )?;
                Ok(())
            },
        )?;
        Ok(())
    }
}

fn run_task<P, F>(
    state: &mut TaskState,
    runtime: &P,
    config: &config::FdbWorkerConfig,
    now_ns: u64,
    interval: Duration,
    mut task: F,
) -> Result<(), WorkerError>
where
    P: HostedRuntimeStore + UniverseStore + 'static,
    F: FnMut(&P, UniverseId, &config::FdbWorkerConfig, u64) -> Result<(), WorkerError>,
{
    if now_ns < state.next_run_ns {
        return Ok(());
    }
    let page_size = config.maintenance_universe_page_size.max(1);
    let universes = universe_page(
        runtime,
        &config.universe_filter,
        state.after_universe,
        page_size,
    )?;
    for universe in &universes {
        task(runtime, *universe, config, now_ns)?;
    }
    state.after_universe = if universes.len() < page_size as usize {
        None
    } else {
        universes.last().copied()
    };
    state.next_run_ns = now_ns.saturating_add(interval_ns(interval));
    Ok(())
}

fn universe_page<P>(
    runtime: &P,
    filter: &BTreeSet<UniverseId>,
    after: Option<UniverseId>,
    limit: u32,
) -> Result<Vec<UniverseId>, WorkerError>
where
    P: UniverseStore + 'static,
{
    if !filter.is_empty() {
        let page = match after {
            Some(cursor) => filter
                .range((Excluded(cursor), Unbounded))
                .take(limit as usize)
                .copied()
                .collect(),
            None => filter.iter().take(limit as usize).copied().collect(),
        };
        return Ok(page);
    }

    Ok(runtime
        .list_universes(after, limit)?
        .into_iter()
        .filter(|record| record.admin.status != UniverseAdminStatus::Deleted)
        .map(|record| record.universe_id)
        .collect())
}

fn interval_ns(duration: Duration) -> u64 {
    duration.as_nanos() as u64
}
