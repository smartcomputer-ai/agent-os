use std::time::{SystemTime, UNIX_EPOCH};

use aos_cbor::to_canonical_cbor;
use serde::Serialize;
use uuid::Uuid;

use aos_kernel::WorldInput;
use aos_node::{
    CborPayload, CommandIngress, CommandRecord, CreateWorldRequest, HostControl, ReceiptIngress,
    SubmissionEnvelope, SubmissionPayload, UniverseId, WorldId, partition_for_world,
    validate_create_world_request,
};

use crate::services::HostedCasService;
use crate::services::{HostedJournalService, HostedMetaService};
use crate::worker::commands::{
    command_submit_response, synthesize_queued_command_record, world_control_from_command_payload,
};
use crate::worker::{CreateWorldAccepted, SubmissionAccepted, SubmitEventRequest, WorkerError};

#[derive(Clone)]
pub struct HostedSubmissionService {
    default_universe_id: UniverseId,
    journal: HostedJournalService,
    meta: HostedMetaService,
    cas: HostedCasService,
    runtime: Option<crate::worker::HostedWorkerRuntime>,
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
        cas: HostedCasService,
    ) -> Self {
        Self {
            default_universe_id,
            journal,
            meta,
            cas,
            runtime: None,
        }
    }

    pub fn from_runtime(
        runtime: crate::worker::HostedWorkerRuntime,
        default_universe_id: UniverseId,
        journal: HostedJournalService,
        meta: HostedMetaService,
        cas: HostedCasService,
    ) -> Self {
        Self {
            default_universe_id,
            journal,
            meta,
            cas,
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
        request: CreateWorldRequest,
    ) -> Result<CreateWorldAccepted, WorkerError> {
        validate_create_world_request(&request)?;
        let world_id = request
            .world_id
            .unwrap_or_else(|| WorldId::from(Uuid::new_v4()));
        let request = CreateWorldRequest {
            world_id: Some(world_id),
            ..request
        };
        let submission_id = format!("create-{world_id}-{}", Uuid::new_v4());
        let submission = SubmissionEnvelope::host_control(
            submission_id.clone(),
            universe_id,
            world_id,
            HostControl::CreateWorld { request },
        );
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
        if let Some(runtime) = &self.runtime {
            return runtime.submit_event(request);
        }
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
            command: None,
            payload: SubmissionPayload::WorldInput {
                input: WorldInput::DomainEvent(aos_wasm_abi::DomainEvent {
                    schema: request.schema,
                    value: serde_cbor::to_vec(&request.value)?,
                    key: None,
                }),
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
        if let Some(runtime) = &self.runtime {
            return runtime.submit_receipt(universe_id, world_id, ingress);
        }
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
            command: None,
            payload: SubmissionPayload::WorldInput {
                input: WorldInput::Receipt(aos_effects::EffectReceipt {
                    intent_hash: ingress.intent_hash.clone().try_into().map_err(|_| {
                        WorkerError::LogFirst(aos_node::BackendError::InvalidIntentHashLen(
                            ingress.intent_hash.len(),
                        ))
                    })?,
                    adapter_id: ingress.adapter_id,
                    status: ingress.status,
                    payload_cbor: resolve_cbor_payload(&ingress.payload)?,
                    cost_cents: ingress.cost_cents,
                    signature: ingress.signature,
                }),
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
        let queued = synthesize_queued_command_record(&ingress);
        let payload_bytes = to_canonical_cbor(payload)?;
        let world_control = world_control_from_command_payload(
            self.resolve_store_for_world(resolved.universe_id)?.as_ref(),
            command,
            &payload_bytes,
        )?;
        let submission = SubmissionEnvelope::world_control(
            format!("cmd-{command_id}"),
            resolved.universe_id,
            world_id,
            resolved.world_epoch,
            ingress,
            world_control,
        );
        let _ = self.journal.submit(submission)?;
        self.meta
            .put_command_record(resolved.universe_id, world_id, queued.clone())?;
        Ok(command_submit_response(world_id, queued))
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

impl HostedSubmissionService {
    fn resolve_store_for_world(
        &self,
        universe_id: UniverseId,
    ) -> Result<std::sync::Arc<crate::blobstore::HostedCas>, WorkerError> {
        self.cas.store_for_domain(universe_id)
    }
}

fn unix_time_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_default()
}

fn resolve_cbor_payload(payload: &CborPayload) -> Result<Vec<u8>, WorkerError> {
    payload.validate()?;
    if let Some(inline) = &payload.inline_cbor {
        return Ok(inline.clone());
    }
    Err(WorkerError::Persist(aos_node::PersistError::validation(
        "standalone hosted submission service requires inline CBOR payloads",
    )))
}
