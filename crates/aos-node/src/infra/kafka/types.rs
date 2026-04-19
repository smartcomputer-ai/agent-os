use std::collections::BTreeMap;

use aos_node::{DEFAULT_JOURNAL_TOPIC, SubmissionRejection, WorldId, WorldLogFrame};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KafkaConfig {
    pub bootstrap_servers: Option<String>,
    pub journal_topic: String,
    pub transactional_id: String,
    pub producer_message_timeout_ms: u32,
    pub producer_flush_timeout_ms: u32,
    pub transaction_timeout_ms: u32,
    pub metadata_timeout_ms: u32,
    pub recovery_fetch_wait_ms: u32,
    pub recovery_poll_interval_ms: u32,
    pub recovery_idle_timeout_ms: u32,
}

impl Default for KafkaConfig {
    fn default() -> Self {
        Self {
            bootstrap_servers: std::env::var("AOS_KAFKA_BOOTSTRAP_SERVERS").ok(),
            journal_topic: std::env::var("AOS_KAFKA_JOURNAL_TOPIC")
                .unwrap_or_else(|_| DEFAULT_JOURNAL_TOPIC.to_owned()),
            transactional_id: std::env::var("AOS_KAFKA_TRANSACTIONAL_ID")
                .unwrap_or_else(|_| "aos-node".to_owned()),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DurableDisposition {
    RejectedSubmission {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        partition: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<i64>,
        world_id: WorldId,
        reason: SubmissionRejection,
    },
    CommandFailure {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        partition: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<i64>,
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
pub struct FetchedRecord {
    pub offset: i64,
    pub key: Option<Vec<u8>>,
    pub value: Option<Vec<u8>>,
}
