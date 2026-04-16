use aos_node::{
    CommandRecord, CreateWorldRequest, ReceiptIngress, UniverseId, WorldId,
    control::AcceptWaitQuery,
};
use serde::Serialize;

use crate::services::{HostedJournalService, HostedMetaService};
use crate::worker::{CreateWorldAccepted, SubmissionAccepted, SubmitEventRequest, WorkerError};

#[derive(Clone)]
pub struct HostedSubmissionService {
    default_universe_id: UniverseId,
    journal: HostedJournalService,
    meta: HostedMetaService,
    runtime: Option<crate::worker::HostedWorkerRuntime>,
}

#[derive(Debug, Clone, Copy)]
struct ResolvedWorld {
    universe_id: UniverseId,
}

impl HostedSubmissionService {
    pub fn new(
        default_universe_id: UniverseId,
        journal: HostedJournalService,
        meta: HostedMetaService,
    ) -> Self {
        Self {
            default_universe_id,
            journal,
            meta,
            runtime: None,
        }
    }

    pub fn from_runtime(
        runtime: crate::worker::HostedWorkerRuntime,
        default_universe_id: UniverseId,
        journal: HostedJournalService,
        meta: HostedMetaService,
    ) -> Self {
        Self {
            default_universe_id,
            journal,
            meta,
            runtime: Some(runtime),
        }
    }

    pub fn default_universe_id(&self) -> Result<UniverseId, WorkerError> {
        Ok(self.default_universe_id)
    }

    pub fn create_world(
        &self,
        universe_id: UniverseId,
        request: CreateWorldRequest,
        wait: AcceptWaitQuery,
    ) -> Result<CreateWorldAccepted, WorkerError> {
        self.create_world_inner(universe_id, request, wait)
    }

    pub fn submit_event(
        &self,
        request: SubmitEventRequest,
        wait: AcceptWaitQuery,
    ) -> Result<SubmissionAccepted, WorkerError> {
        self.submit_event_inner(request, wait)
    }

    pub fn submit_receipt(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        body: ReceiptIngress,
        wait: AcceptWaitQuery,
    ) -> Result<SubmissionAccepted, WorkerError> {
        self.submit_receipt_inner(universe_id, world_id, body, wait)
    }

    pub fn get_command(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, WorkerError> {
        self.get_command_inner(universe_id, world_id, command_id)
    }

    pub fn submit_command<T: Serialize>(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        command: &str,
        command_id: Option<String>,
        actor: Option<String>,
        payload: &T,
        wait: AcceptWaitQuery,
    ) -> Result<aos_node::control::CommandSubmitResponse, WorkerError> {
        self.submit_command_inner(
            universe_id,
            world_id,
            command,
            command_id,
            actor,
            payload,
            wait,
        )
    }
}

impl HostedSubmissionService {
    fn require_runtime(&self) -> Result<&crate::worker::HostedWorkerRuntime, WorkerError> {
        self.runtime.as_ref().ok_or_else(|| {
            WorkerError::Persist(aos_node::PersistError::validation(
                "hosted submissions require a colocated runtime",
            ))
        })
    }

    fn create_world_inner(
        &self,
        universe_id: UniverseId,
        request: CreateWorldRequest,
        wait: AcceptWaitQuery,
    ) -> Result<CreateWorldAccepted, WorkerError> {
        self.require_runtime()?
            .create_world_with_wait(universe_id, request, wait)
    }

    fn submit_event_inner(
        &self,
        request: SubmitEventRequest,
        wait: AcceptWaitQuery,
    ) -> Result<SubmissionAccepted, WorkerError> {
        self.require_runtime()?
            .submit_event_with_wait(request, wait)
    }

    fn submit_receipt_inner(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        ingress: ReceiptIngress,
        wait: AcceptWaitQuery,
    ) -> Result<SubmissionAccepted, WorkerError> {
        self.require_runtime()?
            .submit_receipt_with_wait(universe_id, world_id, ingress, wait)
    }

    fn get_command_inner(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, WorkerError> {
        if let Some(runtime) = &self.runtime {
            return runtime.get_command_record(universe_id, world_id, command_id);
        }
        let resolved = self.resolve_world(universe_id, world_id)?;
        self.meta
            .get_command_record(resolved.universe_id, world_id, command_id)?
            .ok_or_else(|| WorkerError::UnknownCommand {
                universe_id: resolved.universe_id,
                world_id,
                command_id: command_id.to_owned(),
            })
    }

    fn submit_command_inner<T: Serialize>(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        command: &str,
        command_id: Option<String>,
        actor: Option<String>,
        payload: &T,
        wait: AcceptWaitQuery,
    ) -> Result<aos_node::control::CommandSubmitResponse, WorkerError> {
        self.require_runtime()?.submit_command_with_wait(
            universe_id,
            world_id,
            command,
            command_id,
            actor,
            payload,
            wait,
        )
    }

    fn resolve_world(
        &self,
        universe_id_hint: UniverseId,
        world_id: WorldId,
    ) -> Result<ResolvedWorld, WorkerError> {
        self.journal.refresh()?;
        if let Some(frame) = self.journal.world_frames(world_id)?.into_iter().next_back() {
            return Ok(ResolvedWorld {
                universe_id: frame.universe_id,
            });
        }

        if let Some(world) = self
            .meta
            .latest_world_checkpoint(universe_id_hint, world_id)?
        {
            return Ok(ResolvedWorld {
                universe_id: world.universe_id,
            });
        }

        Err(WorkerError::UnknownWorld {
            universe_id: universe_id_hint,
            world_id,
        })
    }
}
