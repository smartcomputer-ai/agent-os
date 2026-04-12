use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash as StdHash, Hasher};

use aos_effects::ReceiptStatus;
use aos_kernel::journal::JournalRecord;
use serde::{Deserialize, Serialize};

use crate::{CborPayload, CommandIngress, CreateWorldRequest, UniverseId, WorldId};

pub const DEFAULT_INGRESS_TOPIC: &str = "aos-ingress";
pub const DEFAULT_JOURNAL_TOPIC: &str = "aos-journal";
pub const SYS_TIMER_FIRED_SCHEMA: &str = "sys/TimerFired@1";

pub type CanonicalWorldRecord = JournalRecord;

pub fn partition_for_world(world_id: WorldId, partition_count: u32) -> u32 {
    assert!(partition_count > 0, "partition_count must be non-zero");
    let mut hasher = DefaultHasher::new();
    world_id.hash(&mut hasher);
    (hasher.finish() % partition_count as u64) as u32
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubmissionPayload {
    DomainEvent {
        schema: String,
        value: CborPayload,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<Vec<u8>>,
    },
    EffectReceipt {
        #[serde(with = "serde_bytes")]
        intent_hash: Vec<u8>,
        adapter_id: String,
        status: ReceiptStatus,
        payload: CborPayload,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost_cents: Option<u64>,
        #[serde(default, with = "serde_bytes")]
        signature: Vec<u8>,
    },
    TimerFired {
        payload: CborPayload,
    },
    Command {
        command: CommandIngress,
    },
    CreateWorld {
        request: CreateWorldRequest,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmissionEnvelope {
    pub submission_id: String,
    pub universe_id: UniverseId,
    pub world_id: WorldId,
    pub world_epoch: u64,
    pub payload: SubmissionPayload,
}

impl SubmissionEnvelope {
    pub fn domain_event(
        submission_id: impl Into<String>,
        universe_id: UniverseId,
        world_id: WorldId,
        world_epoch: u64,
        schema: impl Into<String>,
        value_cbor: Vec<u8>,
    ) -> Self {
        Self {
            submission_id: submission_id.into(),
            universe_id,
            world_id,
            world_epoch,
            payload: SubmissionPayload::DomainEvent {
                schema: schema.into(),
                value: CborPayload::inline(value_cbor),
                key: None,
            },
        }
    }

    pub fn command(
        submission_id: impl Into<String>,
        universe_id: UniverseId,
        world_id: WorldId,
        world_epoch: u64,
        command: CommandIngress,
    ) -> Self {
        Self {
            submission_id: submission_id.into(),
            universe_id,
            world_id,
            world_epoch,
            payload: SubmissionPayload::Command { command },
        }
    }

    pub fn create_world(
        submission_id: impl Into<String>,
        universe_id: UniverseId,
        world_id: WorldId,
        mut request: CreateWorldRequest,
    ) -> Self {
        request.world_id = Some(world_id);
        Self {
            submission_id: submission_id.into(),
            universe_id,
            world_id,
            world_epoch: 1,
            payload: SubmissionPayload::CreateWorld { request },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldLogFrame {
    pub format_version: u16,
    pub universe_id: UniverseId,
    pub world_id: WorldId,
    pub world_epoch: u64,
    pub world_seq_start: u64,
    pub world_seq_end: u64,
    pub records: Vec<CanonicalWorldRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotableBaselineRef {
    pub snapshot_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_manifest_ref: Option<String>,
    pub manifest_hash: String,
    pub height: u64,
    #[serde(default)]
    pub universe_id: crate::UniverseId,
    pub logical_time_ns: u64,
    pub receipt_horizon_height: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldCheckpointRef {
    pub universe_id: UniverseId,
    pub world_id: WorldId,
    pub world_epoch: u64,
    #[serde(default)]
    pub checkpointed_at_ns: u64,
    pub baseline: PromotableBaselineRef,
    pub world_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionCheckpoint {
    pub journal_topic: String,
    pub partition: u32,
    pub journal_offset: u64,
    pub created_at_ns: u64,
    pub worlds: Vec<WorldCheckpointRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionRejection {
    UnknownWorld,
    WorldEpochMismatch { expected: u64, got: u64 },
    DuplicateSubmissionId,
    WorldAlreadyExists,
    InvalidSubmission { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedSubmission {
    pub submission: SubmissionEnvelope,
    pub reason: SubmissionRejection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisteredWorldSummary {
    pub universe_id: UniverseId,
    pub world_id: WorldId,
    pub world_epoch: u64,
    pub effective_partition: u32,
    pub manifest_hash: String,
    pub next_world_seq: u64,
}
