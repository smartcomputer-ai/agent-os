use serde::{Deserialize, Serialize};

use super::identity::InboxSeq;

pub type JournalHeight = u64;
pub type ShardId = u16;
pub type TimeBucket = u64;
pub type QueueSeq = InboxSeq;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CasConfig {
    pub cache_bytes: usize,
    pub cache_item_max_bytes: usize,
    pub verify_reads: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CasLayoutKind {
    #[default]
    Direct,
    Staged,
    Striped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CasRootRecord {
    pub version: u8,
    pub chunk_size: u32,
    pub chunk_count: u32,
    pub size_bytes: u64,
    pub layout_kind: CasLayoutKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CasUploadMarker {
    pub version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer_id: Option<String>,
    #[serde(default)]
    pub started_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_touched_at_ns: Option<u64>,
}

impl Default for CasConfig {
    fn default() -> Self {
        Self {
            cache_bytes: 512 * 1024 * 1024,
            cache_item_max_bytes: 32 * 1024 * 1024,
            verify_reads: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalConfig {
    pub max_batch_entries: usize,
    pub max_batch_bytes: usize,
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self {
            max_batch_entries: 256,
            max_batch_bytes: 256 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboxConfig {
    pub inline_payload_threshold_bytes: usize,
}

impl Default for InboxConfig {
    fn default() -> Self {
        Self {
            inline_payload_threshold_bytes: 4 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DedupeGcConfig {
    pub effect_retention_ns: u64,
    pub timer_retention_ns: u64,
    pub portal_retention_ns: u64,
    pub bucket_width_ns: u64,
}

impl Default for DedupeGcConfig {
    fn default() -> Self {
        Self {
            effect_retention_ns: 60 * 60 * 1_000_000_000,
            timer_retention_ns: 60 * 60 * 1_000_000_000,
            portal_retention_ns: 60 * 60 * 1_000_000_000,
            bucket_width_ns: 5 * 60 * 1_000_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotMaintenanceConfig {
    pub snapshot_after_journal_entries: u64,
    pub segment_target_entries: u64,
    pub segment_hot_tail_margin: u64,
    pub segment_delete_chunk_entries: u32,
}

impl Default for SnapshotMaintenanceConfig {
    fn default() -> Self {
        Self {
            snapshot_after_journal_entries: 1024 * 5,
            segment_target_entries: 1024,
            segment_hot_tail_margin: 64,
            segment_delete_chunk_entries: 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PersistenceConfig {
    pub cas: CasConfig,
    pub journal: JournalConfig,
    pub inbox: InboxConfig,
    pub dedupe_gc: DedupeGcConfig,
    pub snapshot_maintenance: SnapshotMaintenanceConfig,
}
