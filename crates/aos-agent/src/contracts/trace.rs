use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use aos_wasm_sdk::AirSchema;
use serde::{Deserialize, Serialize};

pub const DEFAULT_RUN_TRACE_MAX_ENTRIES: u64 = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/RunTraceEntryKind@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum RunTraceEntryKind {
    #[default]
    RunStarted,
    TurnPlanned,
    LlmRequested,
    LlmReceived,
    ToolCallsObserved,
    ToolBatchPlanned,
    EffectEmitted,
    DomainEventEmitted,
    StreamFrameObserved,
    ReceiptSettled,
    InterventionRequested,
    InterventionApplied,
    ContextOperationStateChanged,
    CompactionRequested,
    CompactionReceived,
    ActiveWindowUpdated,
    RunFinished,
    Custom {
        kind: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/RunTraceRef@1")]
pub struct RunTraceRef {
    pub kind: String,
    #[aos(air_type = "hash")]
    pub ref_: Option<String>,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/RunTraceEntry@1")]
pub struct RunTraceEntry {
    pub seq: u64,
    #[aos(air_type = "time")]
    pub observed_at_ns: u64,
    pub kind: RunTraceEntryKind,
    pub summary: String,
    pub refs: Vec<RunTraceRef>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, AirSchema)]
#[aos(schema = "aos.agent/RunTrace@1")]
pub struct RunTrace {
    pub max_entries: u64,
    pub dropped_entries: u64,
    pub next_seq: u64,
    pub entries: Vec<RunTraceEntry>,
}

impl Default for RunTrace {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_RUN_TRACE_MAX_ENTRIES,
            dropped_entries: 0,
            next_seq: 0,
            entries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/RunTraceSummary@1")]
pub struct RunTraceSummary {
    pub entry_count: u64,
    pub dropped_entries: u64,
    pub first_seq: Option<u64>,
    pub last_seq: Option<u64>,
    pub last_kind: Option<RunTraceEntryKind>,
    pub last_summary: Option<String>,
    #[aos(air_type = "time")]
    pub last_observed_at_ns: Option<u64>,
}
