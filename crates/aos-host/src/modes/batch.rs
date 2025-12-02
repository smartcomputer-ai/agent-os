use aos_store::Store;

use crate::error::HostError;
use crate::host::{CycleOutcome, ExternalEvent, RunMode, WorldHost};

pub struct BatchRunner<S: Store + 'static> {
    host: WorldHost<S>,
}

impl<S: Store + 'static> BatchRunner<S> {
    pub fn new(host: WorldHost<S>) -> Self {
        Self { host }
    }

    pub async fn step(&mut self, events: Vec<ExternalEvent>) -> Result<StepResult, HostError> {
        let events_injected = events.len();
        for evt in events {
            self.host.enqueue_external(evt)?;
        }
        let cycle = self.host.run_cycle(RunMode::Batch).await?;
        self.host.snapshot()?;
        Ok(StepResult {
            cycle,
            events_injected,
        })
    }

    pub fn host(&self) -> &WorldHost<S> {
        &self.host
    }

    pub fn host_mut(&mut self) -> &mut WorldHost<S> {
        &mut self.host
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StepResult {
    pub cycle: CycleOutcome,
    pub events_injected: usize,
}
