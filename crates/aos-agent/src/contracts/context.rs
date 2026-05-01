use super::{LlmTokenCountRecord, LlmUsageRecord, TurnInputLane};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use aos_wasm_sdk::AirSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TranscriptRange@1")]
pub struct TranscriptRange {
    pub start_seq: u64,
    pub end_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ProviderCompatibility@1")]
pub struct ProviderCompatibility {
    pub provider: String,
    pub api_kind: String,
    pub model: Option<String>,
    pub model_family: Option<String>,
    pub artifact_type: String,
    pub opaque: bool,
    pub encrypted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ActiveWindowItemKind@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum ActiveWindowItemKind {
    #[default]
    MessageRef,
    AosSummaryRef,
    ProviderNativeArtifactRef,
    ProviderRawWindowRef,
    Custom {
        kind: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ActiveWindowItem@1")]
pub struct ActiveWindowItem {
    pub item_id: String,
    pub kind: ActiveWindowItemKind,
    #[aos(air_type = "hash")]
    pub ref_: String,
    pub lane: Option<TurnInputLane>,
    pub source_range: Option<TranscriptRange>,
    #[aos(air_type = "hash")]
    pub source_refs: Vec<String>,
    pub provider_compatibility: Option<ProviderCompatibility>,
    pub estimated_tokens: Option<u64>,
    pub metadata: Vec<ContextMetadataEntry>,
}

impl ActiveWindowItem {
    pub fn message_ref(
        item_id: impl Into<String>,
        ref_: impl Into<String>,
        lane: Option<TurnInputLane>,
        estimated_tokens: Option<u64>,
        source_range: Option<TranscriptRange>,
    ) -> Self {
        let ref_ = ref_.into();
        Self {
            item_id: item_id.into(),
            kind: ActiveWindowItemKind::MessageRef,
            ref_: ref_.clone(),
            lane,
            source_range,
            source_refs: vec![ref_],
            provider_compatibility: None,
            estimated_tokens,
            metadata: Vec::new(),
        }
    }

    pub fn renderable_message_ref(&self) -> Option<&str> {
        match self.kind {
            ActiveWindowItemKind::MessageRef | ActiveWindowItemKind::AosSummaryRef => {
                Some(self.ref_.as_str())
            }
            ActiveWindowItemKind::ProviderNativeArtifactRef
            | ActiveWindowItemKind::ProviderRawWindowRef
            | ActiveWindowItemKind::Custom { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextMetadataEntry@1")]
pub struct ContextMetadataEntry {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TranscriptLedgerEntryKind@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum TranscriptLedgerEntryKind {
    #[default]
    MessageRef,
    SummaryRef,
    ProviderArtifactRef,
    Custom {
        kind: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TranscriptLedgerEntry@1")]
pub struct TranscriptLedgerEntry {
    pub seq: u64,
    pub kind: TranscriptLedgerEntryKind,
    #[aos(air_type = "hash")]
    pub ref_: String,
    pub source: String,
    #[aos(air_type = "time")]
    pub appended_at_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TranscriptLedger@1")]
pub struct TranscriptLedger {
    pub next_seq: u64,
    pub entries: Vec<TranscriptLedgerEntry>,
}

impl TranscriptLedger {
    pub fn message_refs(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| matches!(entry.kind, TranscriptLedgerEntryKind::MessageRef))
            .map(|entry| entry.ref_.clone())
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/CompactionStrategy@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum CompactionStrategy {
    ProviderNative,
    AosSummary,
    #[default]
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/CompactionArtifactKind@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum CompactionArtifactKind {
    AosSummary,
    ProviderNative,
    #[default]
    Mixed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextPressureReason@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum ContextPressureReason {
    ProviderContextLimit,
    ProviderRecommended,
    UsageHighWater,
    LocalWindowPolicy,
    Manual,
    CountTokensOverBudget,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextPressureRecord@1")]
pub struct ContextPressureRecord {
    pub reason: ContextPressureReason,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub candidate_plan_id: Option<String>,
    pub observed_usage: Option<LlmUsageRecord>,
    pub error_kind: Option<String>,
    #[aos(air_type = "hash")]
    pub error_ref: Option<String>,
    pub recommended_strategy: Option<CompactionStrategy>,
    #[aos(air_type = "time")]
    pub observed_at_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextOperationPhase@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum ContextOperationPhase {
    #[default]
    Idle,
    NeedsCompaction,
    CountingTokens,
    Compacting,
    ApplyingCompaction,
    Failed,
}

impl ContextOperationPhase {
    pub fn blocks_generation(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Idle => "Idle",
            Self::NeedsCompaction => "NeedsCompaction",
            Self::CountingTokens => "CountingTokens",
            Self::Compacting => "Compacting",
            Self::ApplyingCompaction => "ApplyingCompaction",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextOperationState@1")]
pub struct ContextOperationState {
    pub operation_id: String,
    pub phase: ContextOperationPhase,
    pub reason: ContextPressureReason,
    pub candidate_plan_id: Option<String>,
    pub strategy: CompactionStrategy,
    pub source_range: Option<TranscriptRange>,
    #[aos(air_type = "hash")]
    pub source_items_ref: Option<String>,
    #[aos(air_type = "hash")]
    pub effect_intent_id: Option<String>,
    #[aos(air_type = "hash")]
    pub params_hash: Option<String>,
    pub failure: Option<String>,
    #[aos(air_type = "time")]
    pub started_at_ns: u64,
    #[aos(air_type = "time")]
    pub updated_at_ns: u64,
}

impl ContextOperationState {
    pub fn needs_compaction(
        operation_id: impl Into<String>,
        reason: ContextPressureReason,
        strategy: CompactionStrategy,
        source_range: Option<TranscriptRange>,
        now_ns: u64,
    ) -> Self {
        Self {
            operation_id: operation_id.into(),
            phase: ContextOperationPhase::NeedsCompaction,
            reason,
            candidate_plan_id: None,
            strategy,
            source_range,
            source_items_ref: None,
            effect_intent_id: None,
            params_hash: None,
            failure: None,
            started_at_ns: now_ns,
            updated_at_ns: now_ns,
        }
    }

    pub fn blocks_generation(&self) -> bool {
        self.phase.blocks_generation()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/CompactionRecord@1")]
pub struct CompactionRecord {
    pub operation_id: String,
    pub strategy: CompactionStrategy,
    pub artifact_kind: CompactionArtifactKind,
    #[aos(air_type = "hash")]
    pub artifact_refs: Vec<String>,
    pub source_range: TranscriptRange,
    #[aos(air_type = "hash")]
    pub source_refs: Vec<String>,
    pub active_window_items: Vec<ActiveWindowItem>,
    pub provider_compatibility: Option<ProviderCompatibility>,
    pub usage: Option<LlmUsageRecord>,
    #[aos(air_type = "time")]
    pub created_at_ns: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextState@1")]
pub struct ContextState {
    pub transcript_ledger: TranscriptLedger,
    pub active_window_items: Vec<ActiveWindowItem>,
    pub compaction_records: Vec<CompactionRecord>,
    pub compacted_through: Option<u64>,
    pub pending_context_operation: Option<ContextOperationState>,
    pub last_llm_usage: Option<LlmUsageRecord>,
    pub last_context_pressure: Option<ContextPressureRecord>,
    pub last_token_count: Option<LlmTokenCountRecord>,
    pub last_compaction: Option<CompactionRecord>,
}

impl ContextState {
    pub fn set_pending_operation(&mut self, operation: ContextOperationState) {
        if matches!(operation.phase, ContextOperationPhase::Idle) {
            self.pending_context_operation = None;
        } else {
            self.pending_context_operation = Some(operation);
        }
    }

    pub fn clear_pending_operation(&mut self) {
        self.pending_context_operation = None;
    }

    pub fn append_message_refs(
        &mut self,
        refs: impl IntoIterator<Item = String>,
        source: &str,
        appended_at_ns: u64,
    ) -> Vec<ActiveWindowItem> {
        let mut items = Vec::new();
        for ref_ in refs {
            let seq = self.transcript_ledger.next_seq;
            self.transcript_ledger.next_seq = self.transcript_ledger.next_seq.saturating_add(1);
            self.transcript_ledger.entries.push(TranscriptLedgerEntry {
                seq,
                kind: TranscriptLedgerEntryKind::MessageRef,
                ref_: ref_.clone(),
                source: source.into(),
                appended_at_ns,
            });
            let item = ActiveWindowItem::message_ref(
                alloc::format!("ledger:{seq}"),
                ref_,
                Some(TurnInputLane::Conversation),
                None,
                Some(TranscriptRange {
                    start_seq: seq,
                    end_seq: seq.saturating_add(1),
                }),
            );
            self.active_window_items.push(item.clone());
            items.push(item);
        }
        items
    }

    pub fn ledger_message_refs(&self) -> Vec<String> {
        self.transcript_ledger.message_refs()
    }
}
