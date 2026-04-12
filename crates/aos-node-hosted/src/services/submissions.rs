use std::time::{SystemTime, UNIX_EPOCH};

use aos_cbor::to_canonical_cbor;
use serde::Serialize;
use uuid::Uuid;

use aos_node::{
    CborPayload, CommandIngress, CommandRecord, CommandStatus, CreateWorldRequest, ReceiptIngress,
    SubmissionEnvelope, SubmissionPayload, UniverseId, WorldId, partition_for_world,
};

use crate::services::{HostedJournalService, HostedMetaService};
use crate::worker::{CreateWorldAccepted, SubmissionAccepted, SubmitEventRequest, WorkerError};

#[derive(Clone)]
pub struct HostedSubmissionService {
    default_universe_id: UniverseId,
    journal: HostedJournalService,
    meta: HostedMetaService,
}

#[derive(Debug, Clone, Copy)]
struct ResolvedWorld {
    universe_id: UniverseId,
    world_epoch: u64,
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
        }
    }

    pub fn default_universe_id(&self) -> Result<UniverseId, WorkerError> {
        Ok(self.default_universe_id)
    }

    pub fn create_world(
        &self,
        universe_id: UniverseId,
        request: CreateWorldRequest,
    ) -> Result<CreateWorldAccepted, WorkerError> {
        self.create_world_inner(universe_id, request)
    }

    pub fn submit_event(
        &self,
        request: SubmitEventRequest,
    ) -> Result<SubmissionAccepted, WorkerError> {
        self.submit_event_inner(request)
    }

    pub fn submit_receipt(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        body: ReceiptIngress,
    ) -> Result<SubmissionAccepted, WorkerError> {
        self.submit_receipt_inner(universe_id, world_id, body)
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
    ) -> Result<aos_node::api::CommandSubmitResponse, WorkerError> {
        self.submit_command_inner(universe_id, world_id, command, command_id, actor, payload)
    }
}

impl HostedSubmissionService {
    fn create_world_inner(
        &self,
        universe_id: UniverseId,
        mut request: CreateWorldRequest,
    ) -> Result<CreateWorldAccepted, WorkerError> {
        let world_id = request
            .world_id
            .unwrap_or_else(|| WorldId::from(Uuid::new_v4()));
        request.universe_id = universe_id;
        request.world_id = Some(world_id);

        let submission_id = format!("create-{}", Uuid::new_v4());
        let submission =
            SubmissionEnvelope::create_world(submission_id.clone(), universe_id, world_id, request);
        let submission_offset = self.journal.submit(submission)?;
        Ok(CreateWorldAccepted {
            submission_id,
            submission_offset,
            world_id,
            effective_partition: partition_for_world(world_id, self.journal.partition_count()?),
        })
    }

    fn submit_event_inner(
        &self,
        request: SubmitEventRequest,
    ) -> Result<SubmissionAccepted, WorkerError> {
        let resolved = self.resolve_world(request.universe_id, request.world_id)?;
        if let Some(expected_world_epoch) = request.expected_world_epoch
            && expected_world_epoch != resolved.world_epoch
        {
            return Err(WorkerError::WorldEpochMismatch {
                universe_id: request.universe_id,
                world_id: request.world_id,
                expected: resolved.world_epoch,
                got: expected_world_epoch,
            });
        }

        let submission_id = request
            .submission_id
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let submission = SubmissionEnvelope {
            submission_id: submission_id.clone(),
            universe_id: resolved.universe_id,
            world_id: request.world_id,
            world_epoch: resolved.world_epoch,
            payload: SubmissionPayload::DomainEvent {
                schema: request.schema,
                value: CborPayload::inline(serde_cbor::to_vec(&request.value)?),
                key: None,
            },
        };
        let submission_offset = self.journal.submit(submission)?;
        Ok(SubmissionAccepted {
            submission_id,
            submission_offset,
            world_epoch: resolved.world_epoch,
            effective_partition: partition_for_world(
                request.world_id,
                self.journal.partition_count()?,
            ),
        })
    }

    fn submit_receipt_inner(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        ingress: ReceiptIngress,
    ) -> Result<SubmissionAccepted, WorkerError> {
        let resolved = self.resolve_world(universe_id, world_id)?;
        let submission_id = ingress
            .correlation_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let submission = SubmissionEnvelope {
            submission_id: submission_id.clone(),
            universe_id: resolved.universe_id,
            world_id,
            world_epoch: resolved.world_epoch,
            payload: SubmissionPayload::EffectReceipt {
                intent_hash: ingress.intent_hash,
                adapter_id: ingress.adapter_id,
                status: ingress.status,
                payload: ingress.payload,
                cost_cents: ingress.cost_cents,
                signature: ingress.signature,
            },
        };
        let submission_offset = self.journal.submit(submission)?;
        Ok(SubmissionAccepted {
            submission_id,
            submission_offset,
            world_epoch: resolved.world_epoch,
            effective_partition: partition_for_world(world_id, self.journal.partition_count()?),
        })
    }

    fn get_command_inner(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, WorkerError> {
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
    ) -> Result<aos_node::api::CommandSubmitResponse, WorkerError> {
        let resolved = self.resolve_world(universe_id, world_id)?;
        if let Some(existing_id) = command_id.as_deref()
            && let Some(existing) =
                self.meta
                    .get_command_record(resolved.universe_id, world_id, existing_id)?
        {
            return Ok(command_submit_response(world_id, existing));
        }

        let command_id = command_id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let submitted_at_ns = unix_time_ns();
        let ingress = CommandIngress {
            command_id: command_id.clone(),
            command: command.to_owned(),
            actor,
            payload: CborPayload::inline(to_canonical_cbor(payload)?),
            submitted_at_ns,
        };
        let submission = SubmissionEnvelope::command(
            command_id.clone(),
            resolved.universe_id,
            world_id,
            resolved.world_epoch,
            ingress,
        );
        let _submission_offset = self.journal.submit(submission)?;

        let queued = CommandRecord {
            command_id: command_id.clone(),
            command: command.to_owned(),
            status: CommandStatus::Queued,
            submitted_at_ns,
            started_at_ns: None,
            finished_at_ns: None,
            journal_height: None,
            manifest_hash: None,
            result_payload: None,
            error: None,
        };
        let record =
            match self
                .meta
                .get_command_record(resolved.universe_id, world_id, &command_id)?
            {
                Some(existing) if existing.status != CommandStatus::Queued => existing,
                _ => {
                    self.meta
                        .put_command_record(resolved.universe_id, world_id, queued.clone())?;
                    queued
                }
            };
        Ok(command_submit_response(world_id, record))
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
                world_epoch: frame.world_epoch,
            });
        }

        let partition = partition_for_world(world_id, self.journal.partition_count()?);
        if let Some(checkpoint) = self.meta.latest_checkpoint(universe_id_hint, partition)?
            && let Some(world) = checkpoint
                .worlds
                .into_iter()
                .find(|world| world.world_id == world_id)
        {
            return Ok(ResolvedWorld {
                universe_id: world.universe_id,
                world_epoch: world.world_epoch,
            });
        }

        Err(WorkerError::UnknownWorld {
            universe_id: universe_id_hint,
            world_id,
        })
    }
}

fn unix_time_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_default()
}

fn command_submit_response(
    world_id: WorldId,
    record: CommandRecord,
) -> aos_node::api::CommandSubmitResponse {
    aos_node::api::CommandSubmitResponse {
        poll_url: format!("/v1/worlds/{world_id}/commands/{}", record.command_id),
        command_id: record.command_id,
        status: record.status,
    }
}
