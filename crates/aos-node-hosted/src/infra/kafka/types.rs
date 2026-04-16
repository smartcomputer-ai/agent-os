use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use aos_node::{
    DEFAULT_INGRESS_TOPIC, DEFAULT_JOURNAL_TOPIC, SubmissionEnvelope, SubmissionRejection, WorldId,
    WorldLogFrame,
};
use rdkafka::consumer::ConsumerGroupMetadata;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KafkaConfig {
    pub bootstrap_servers: Option<String>,
    pub ingress_topic: String,
    pub journal_topic: String,
    pub projection_topic: String,
    pub submission_group_prefix: String,
    pub transactional_id: String,
    pub direct_assigned_partitions: BTreeSet<u32>,
    pub direct_assignment_start_from_end: bool,
    pub producer_message_timeout_ms: u32,
    pub producer_flush_timeout_ms: u32,
    pub transaction_timeout_ms: u32,
    pub metadata_timeout_ms: u32,
    pub group_session_timeout_ms: u32,
    pub group_heartbeat_interval_ms: u32,
    pub group_poll_wait_ms: u32,
    pub max_pending_ingress_per_partition: usize,
    pub recovery_fetch_wait_ms: u32,
    pub recovery_poll_interval_ms: u32,
    pub recovery_idle_timeout_ms: u32,
}

impl Default for KafkaConfig {
    fn default() -> Self {
        Self {
            bootstrap_servers: std::env::var("AOS_KAFKA_BOOTSTRAP_SERVERS").ok(),
            ingress_topic: std::env::var("AOS_KAFKA_INGRESS_TOPIC")
                .unwrap_or_else(|_| DEFAULT_INGRESS_TOPIC.to_owned()),
            journal_topic: std::env::var("AOS_KAFKA_JOURNAL_TOPIC")
                .unwrap_or_else(|_| DEFAULT_JOURNAL_TOPIC.to_owned()),
            projection_topic: std::env::var("AOS_KAFKA_PROJECTION_TOPIC")
                .unwrap_or_else(|_| "aos-projection".to_owned()),
            submission_group_prefix: std::env::var("AOS_KAFKA_GROUP_PREFIX")
                .unwrap_or_else(|_| "aos-node-hosted".to_owned()),
            transactional_id: std::env::var("AOS_KAFKA_TRANSACTIONAL_ID")
                .unwrap_or_else(|_| "aos-node-hosted".to_owned()),
            direct_assigned_partitions: BTreeSet::new(),
            direct_assignment_start_from_end: false,
            producer_message_timeout_ms: std::env::var("AOS_KAFKA_PRODUCER_MESSAGE_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(2_000),
            producer_flush_timeout_ms: std::env::var("AOS_KAFKA_PRODUCER_FLUSH_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(2_000),
            transaction_timeout_ms: std::env::var("AOS_KAFKA_TRANSACTION_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(5_000),
            metadata_timeout_ms: std::env::var("AOS_KAFKA_METADATA_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(2_000),
            group_session_timeout_ms: std::env::var("AOS_KAFKA_GROUP_SESSION_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(6_000),
            group_heartbeat_interval_ms: std::env::var("AOS_KAFKA_GROUP_HEARTBEAT_INTERVAL_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(300),
            group_poll_wait_ms: std::env::var("AOS_KAFKA_GROUP_POLL_WAIT_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(5),
            max_pending_ingress_per_partition: std::env::var(
                "AOS_KAFKA_MAX_PENDING_INGRESS_PER_PARTITION",
            )
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1024),
            recovery_fetch_wait_ms: std::env::var("AOS_KAFKA_RECOVERY_FETCH_WAIT_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(50),
            recovery_poll_interval_ms: std::env::var("AOS_KAFKA_RECOVERY_POLL_INTERVAL_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(25),
            recovery_idle_timeout_ms: std::env::var("AOS_KAFKA_RECOVERY_IDLE_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(100),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionLogEntry {
    pub offset: u64,
    pub frame: WorldLogFrame,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionTopicEntry {
    pub offset: u64,
    pub key: Vec<u8>,
    pub value: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct IngressRecord {
    pub partition: u32,
    pub offset: i64,
    pub envelope: SubmissionEnvelope,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssignmentSync {
    pub assigned: Vec<u32>,
    pub newly_assigned: Vec<u32>,
    pub revoked: Vec<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct IngressPollBatch {
    pub assignment: AssignmentSync,
    pub records: Vec<IngressRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DurableDisposition {
    RejectedSubmission {
        partition: u32,
        offset: i64,
        world_id: WorldId,
        reason: SubmissionRejection,
    },
    CommandFailure {
        partition: u32,
        offset: i64,
        world_id: WorldId,
        command_id: String,
        error_code: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostedJournalRecord {
    Frame(WorldLogFrame),
    Disposition(DurableDisposition),
}

#[derive(Debug, Clone, Default)]
pub struct FlushCommit {
    pub frames: Vec<WorldLogFrame>,
    pub dispositions: Vec<DurableDisposition>,
    pub offset_commits: BTreeMap<u32, i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CommitFailpoint {
    AbortBeforeCommit,
}

#[derive(Debug)]
pub(super) struct QueuedSubmission {
    pub offset: i64,
    pub submission: SubmissionEnvelope,
}

pub(super) type SharedConsumerGroupMetadata = Arc<Mutex<Option<ConsumerGroupMetadata>>>;

#[derive(Debug)]
pub struct FetchedRecord {
    pub offset: i64,
    pub key: Option<Vec<u8>>,
    pub value: Option<Vec<u8>>,
}
