use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use crate::api::{
    DefGetResponse, DefsListResponse, HeadInfoResponse, JournalEntriesResponse, ManifestResponse,
    RawJournalEntriesResponse, StateGetResponse, StateListResponse, WorkspaceResolveResponse,
    WorldSummaryResponse,
};
use crate::{
    CommandRecord, DomainEventIngress, ReceiptIngress, WorkerHeartbeat, WorldId, WorldRuntimeInfo,
};
use aos_air_types::AirNode;
use aos_runtime::trace::TraceQuery;
use thiserror::Error;

use crate::api::ControlError;

use super::ingress::LocalIngressQueue;
use super::runner::{LocalWorker, LocalWorkerOutcome};
use super::runtime::{LocalLogRuntime, LocalRuntimeError};

#[derive(Debug, Clone)]
pub struct LocalSupervisorConfig {
    pub poll_interval: Duration,
}

impl Default for LocalSupervisorConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(100),
        }
    }
}

#[derive(Debug, Error)]
pub enum LocalNodeError {
    #[error(transparent)]
    Runtime(#[from] LocalRuntimeError),
}

impl From<LocalNodeError> for ControlError {
    fn from(value: LocalNodeError) -> Self {
        match value {
            LocalNodeError::Runtime(err) => ControlError::from(err),
        }
    }
}

pub struct LocalSupervisor {
    runtime: Arc<LocalLogRuntime>,
    ingress: Arc<LocalIngressQueue>,
    worker: LocalWorker,
    config: LocalSupervisorConfig,
    shutdown: AtomicBool,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl LocalSupervisor {
    pub fn new(runtime: Arc<LocalLogRuntime>, config: LocalSupervisorConfig) -> Arc<Self> {
        let ingress = Arc::new(LocalIngressQueue::default());
        Arc::new(Self {
            worker: LocalWorker::new(runtime.clone(), ingress.clone()),
            runtime,
            ingress,
            config,
            shutdown: AtomicBool::new(false),
            thread: Mutex::new(None),
        })
    }

    pub fn start(self: &Arc<Self>) {
        let mut thread_guard = self.thread.lock().expect("local supervisor mutex poisoned");
        if thread_guard.is_some() {
            return;
        }
        let this = Arc::clone(self);
        *thread_guard = Some(thread::spawn(move || {
            while !this.shutdown.load(Ordering::Relaxed) {
                let _ = this.worker.run_once();
                thread::sleep(this.config.poll_interval);
            }
        }));
    }

    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self
            .thread
            .lock()
            .expect("local supervisor mutex poisoned")
            .take()
        {
            let _ = handle.join();
        }
    }

    pub fn worker_heartbeat(&self, worker_id: &str) -> WorkerHeartbeat {
        let now_ns = aos_runtime::now_wallclock_ns();
        WorkerHeartbeat {
            worker_id: worker_id.to_string(),
            pins: Vec::new(),
            last_seen_ns: now_ns,
            expires_at_ns: u64::MAX,
        }
    }

    pub fn world_summary(&self, world: WorldId) -> Result<WorldSummaryResponse, LocalNodeError> {
        let (runtime, active_baseline) = self.runtime.world_summary(world)?;
        Ok(WorldSummaryResponse {
            runtime,
            active_baseline,
        })
    }

    pub fn list_worlds(
        &self,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, LocalNodeError> {
        Ok(self.runtime.list_worlds(after, limit)?)
    }

    pub fn worker_worlds(
        &self,
        worker_id: &str,
        limit: u32,
        local_worker_id: &str,
    ) -> Result<Vec<WorldRuntimeInfo>, LocalNodeError> {
        if worker_id != local_worker_id {
            return Ok(Vec::new());
        }
        Ok(self.runtime.worker_worlds(limit)?)
    }

    pub fn runtime_info(&self, world: WorldId) -> Result<WorldRuntimeInfo, LocalNodeError> {
        Ok(self.runtime.world_runtime(world)?)
    }

    pub fn manifest(&self, world: WorldId) -> Result<ManifestResponse, LocalNodeError> {
        Ok(self.runtime.manifest(world)?)
    }

    pub fn defs_list(
        &self,
        world: WorldId,
        kinds: Option<Vec<String>>,
        prefix: Option<String>,
    ) -> Result<DefsListResponse, LocalNodeError> {
        Ok(self.runtime.defs_list(world, kinds, prefix)?)
    }

    pub fn def_get(&self, world: WorldId, name: &str) -> Result<DefGetResponse, LocalNodeError> {
        Ok(self.runtime.def_get(world, name)?)
    }

    pub fn state_get(
        &self,
        world: WorldId,
        workflow: &str,
        key: Option<Vec<u8>>,
    ) -> Result<StateGetResponse, LocalNodeError> {
        Ok(self.runtime.state_get(world, workflow, key)?)
    }

    pub fn state_list(
        &self,
        world: WorldId,
        workflow: &str,
        limit: u32,
    ) -> Result<StateListResponse, LocalNodeError> {
        Ok(self.runtime.state_list(world, workflow, limit)?)
    }

    pub fn enqueue_event(
        &self,
        world: WorldId,
        ingress: DomainEventIngress,
    ) -> Result<crate::InboxSeq, LocalNodeError> {
        let submission = self.runtime.build_event_submission(world, ingress)?;
        Ok(self.ingress.enqueue(submission))
    }

    pub fn enqueue_receipt(
        &self,
        world: WorldId,
        ingress: ReceiptIngress,
    ) -> Result<crate::InboxSeq, LocalNodeError> {
        let submission = self.runtime.build_receipt_submission(world, ingress)?;
        Ok(self.ingress.enqueue(submission))
    }

    pub fn get_command(
        &self,
        world: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, LocalNodeError> {
        Ok(self.runtime.get_command(world, command_id)?)
    }

    pub fn submit_command<T: serde::Serialize>(
        &self,
        world: WorldId,
        command: &str,
        command_id: Option<String>,
        actor: Option<String>,
        payload: &T,
    ) -> Result<CommandRecord, LocalNodeError> {
        let (submission, record) = self
            .runtime
            .queue_command_submission(world, command, command_id, actor, payload)?;
        if let Some(submission) = submission {
            let _ = self.ingress.enqueue(submission);
        }
        Ok(record)
    }

    pub fn journal_head(&self, world: WorldId) -> Result<HeadInfoResponse, LocalNodeError> {
        Ok(self.runtime.journal_head(world)?)
    }

    pub fn journal_entries(
        &self,
        world: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<JournalEntriesResponse, LocalNodeError> {
        Ok(self.runtime.journal_entries(world, from, limit)?)
    }

    pub fn journal_entries_raw(
        &self,
        world: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<RawJournalEntriesResponse, LocalNodeError> {
        Ok(self.runtime.journal_entries_raw(world, from, limit)?)
    }

    pub fn trace(
        &self,
        world: WorldId,
        query: TraceQuery,
    ) -> Result<serde_json::Value, LocalNodeError> {
        Ok(self.runtime.trace(world, query)?)
    }

    pub fn trace_summary(&self, world: WorldId) -> Result<serde_json::Value, LocalNodeError> {
        Ok(self.runtime.trace_summary(world)?)
    }

    pub fn workspace_resolve(
        &self,
        world: WorldId,
        workspace: &str,
        version: Option<u64>,
    ) -> Result<WorkspaceResolveResponse, LocalNodeError> {
        Ok(self.runtime.workspace_resolve(world, workspace, version)?)
    }

    pub fn runtime(&self) -> &Arc<LocalLogRuntime> {
        &self.runtime
    }

    pub fn ingress(&self) -> &Arc<LocalIngressQueue> {
        &self.ingress
    }

    pub fn run_once(&self) -> Result<LocalWorkerOutcome, LocalNodeError> {
        Ok(self.worker.run_once()?)
    }
}

pub fn def_matches_kind(def: &AirNode, kind: &str) -> bool {
    let _ = def;
    let _ = kind;
    true
}
