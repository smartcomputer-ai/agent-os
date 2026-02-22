use std::collections::{HashSet, VecDeque};

use aos_effects::{EffectIntent, EffectKind as RuntimeEffectKind};
use serde::{Deserialize, Serialize};
use serde_bytes;

use crate::journal::JournalSeq;
use crate::plan::{PlanCompletionValue, PlanInstanceSnapshot};
use crate::receipts::ReducerEffectContext;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelSnapshot {
    reducer_state: Vec<ReducerStateEntry>,
    reducer_index_roots: Vec<(String, [u8; 32])>,
    recent_receipts: Vec<[u8; 32]>,
    plan_instances: Vec<PlanInstanceSnapshot>,
    pending_plan_receipts: Vec<PendingPlanReceiptSnapshot>,
    waiting_events: Vec<(String, Vec<u64>)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    plan_wait_watchers: Vec<(u64, Vec<u64>)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    completed_plan_outcomes: Vec<PlanCompletionSnapshot>,
    next_plan_id: u64,
    queued_effects: Vec<EffectIntentSnapshot>,
    pending_reducer_receipts: Vec<ReducerReceiptSnapshot>,
    plan_results: Vec<PlanResultSnapshot>,
    height: JournalSeq,
    #[serde(default)]
    logical_now_ns: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    manifest_hash: Option<Vec<u8>>, // CBOR-encoded hash bytes (sha256)
    #[serde(default)]
    root_completeness: SnapshotRootCompleteness,
}

impl KernelSnapshot {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        height: JournalSeq,
        reducer_state: Vec<ReducerStateEntry>,
        recent_receipts: Vec<[u8; 32]>,
        plan_instances: Vec<PlanInstanceSnapshot>,
        pending_plan_receipts: Vec<PendingPlanReceiptSnapshot>,
        waiting_events: Vec<(String, Vec<u64>)>,
        next_plan_id: u64,
        queued_effects: Vec<EffectIntentSnapshot>,
        pending_reducer_receipts: Vec<ReducerReceiptSnapshot>,
        plan_results: Vec<PlanResultSnapshot>,
        logical_now_ns: u64,
        manifest_hash: Option<[u8; 32]>,
    ) -> Self {
        Self {
            reducer_state,
            reducer_index_roots: Vec::new(),
            recent_receipts,
            plan_instances,
            pending_plan_receipts,
            waiting_events,
            plan_wait_watchers: Vec::new(),
            completed_plan_outcomes: Vec::new(),
            next_plan_id,
            queued_effects,
            pending_reducer_receipts,
            plan_results,
            height,
            logical_now_ns,
            manifest_hash: manifest_hash.map(|h| h.to_vec()),
            root_completeness: SnapshotRootCompleteness::default(),
        }
    }

    pub fn reducer_state_entries(&self) -> &[ReducerStateEntry] {
        &self.reducer_state
    }

    pub fn reducer_index_roots(&self) -> &[(String, [u8; 32])] {
        &self.reducer_index_roots
    }

    pub fn set_reducer_index_roots(&mut self, roots: Vec<(String, [u8; 32])>) {
        self.reducer_index_roots = roots;
    }

    pub fn recent_receipts(&self) -> &[[u8; 32]] {
        &self.recent_receipts
    }

    pub fn height(&self) -> JournalSeq {
        self.height
    }

    pub fn plan_instances(&self) -> &[PlanInstanceSnapshot] {
        &self.plan_instances
    }

    pub fn pending_plan_receipts(&self) -> &[PendingPlanReceiptSnapshot] {
        &self.pending_plan_receipts
    }

    pub fn waiting_events(&self) -> &[(String, Vec<u64>)] {
        &self.waiting_events
    }

    pub fn plan_wait_watchers(&self) -> &[(u64, Vec<u64>)] {
        &self.plan_wait_watchers
    }

    pub fn set_plan_wait_watchers(&mut self, watchers: Vec<(u64, Vec<u64>)>) {
        self.plan_wait_watchers = watchers;
    }

    pub fn completed_plan_outcomes(&self) -> &[PlanCompletionSnapshot] {
        &self.completed_plan_outcomes
    }

    pub fn set_completed_plan_outcomes(&mut self, outcomes: Vec<PlanCompletionSnapshot>) {
        self.completed_plan_outcomes = outcomes;
    }

    pub fn next_plan_id(&self) -> u64 {
        self.next_plan_id
    }

    pub fn queued_effects(&self) -> &[EffectIntentSnapshot] {
        &self.queued_effects
    }

    pub fn pending_reducer_receipts(&self) -> &[ReducerReceiptSnapshot] {
        &self.pending_reducer_receipts
    }

    pub fn plan_results(&self) -> &[PlanResultSnapshot] {
        &self.plan_results
    }

    pub fn logical_now_ns(&self) -> u64 {
        self.logical_now_ns
    }

    pub fn manifest_hash(&self) -> Option<&[u8]> {
        self.manifest_hash.as_deref()
    }

    pub fn set_root_completeness(&mut self, roots: SnapshotRootCompleteness) {
        self.root_completeness = roots;
    }

    pub fn root_completeness(&self) -> &SnapshotRootCompleteness {
        &self.root_completeness
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SnapshotRootCompleteness {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub manifest_hash: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reducer_state_roots: Vec<[u8; 32]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cell_index_roots: Vec<[u8; 32]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_roots: Vec<[u8; 32]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_roots: Vec<[u8; 32]>,
}

pub fn receipts_to_vecdeque(
    receipts: &[[u8; 32]],
    cap: usize,
) -> (VecDeque<[u8; 32]>, HashSet<[u8; 32]>) {
    let mut deque = VecDeque::new();
    let mut set = HashSet::new();
    for hash in receipts.iter().cloned().take(cap) {
        deque.push_back(hash);
        set.insert(hash);
    }
    (deque, set)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectIntentSnapshot {
    pub intent_hash: [u8; 32],
    pub kind: String,
    pub cap_name: String,
    #[serde(with = "serde_bytes")]
    pub params_cbor: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub idempotency_key: [u8; 32],
}

impl EffectIntentSnapshot {
    pub fn from_intent(intent: &EffectIntent) -> Self {
        Self {
            intent_hash: intent.intent_hash,
            kind: intent.kind.as_str().to_string(),
            cap_name: intent.cap_name.clone(),
            params_cbor: intent.params_cbor.clone(),
            idempotency_key: intent.idempotency_key,
        }
    }

    pub fn into_intent(self) -> EffectIntent {
        EffectIntent {
            kind: RuntimeEffectKind::new(self.kind),
            cap_name: self.cap_name,
            params_cbor: self.params_cbor,
            idempotency_key: self.idempotency_key,
            intent_hash: self.intent_hash,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPlanReceiptSnapshot {
    pub plan_id: u64,
    pub intent_hash: [u8; 32],
    pub effect_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanCompletionSnapshot {
    pub plan_id: u64,
    pub value: PlanCompletionValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReducerReceiptSnapshot {
    pub intent_hash: [u8; 32],
    pub reducer: String,
    pub effect_kind: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub key: Option<Vec<u8>>,
    #[serde(with = "serde_bytes")]
    pub params_cbor: Vec<u8>,
}

impl ReducerReceiptSnapshot {
    pub fn from_context(intent_hash: [u8; 32], ctx: &ReducerEffectContext) -> Self {
        Self {
            intent_hash,
            reducer: ctx.reducer.clone(),
            effect_kind: ctx.effect_kind.clone(),
            key: ctx.key.clone(),
            params_cbor: ctx.params_cbor.clone(),
        }
    }

    pub fn into_context(self) -> ReducerEffectContext {
        ReducerEffectContext::new(self.reducer, self.effect_kind, self.params_cbor, self.key)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanResultSnapshot {
    pub plan_name: String,
    pub plan_id: u64,
    pub output_schema: String,
    #[serde(with = "serde_bytes")]
    pub value_cbor: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReducerStateEntry {
    pub reducer: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub key: Option<Vec<u8>>,
    #[serde(with = "serde_bytes")]
    pub state: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub state_hash: [u8; 32],
    pub last_active_ns: u64,
}

mod serde_bytes_opt {
    use serde::{Deserialize, Deserializer, Serializer};
    use serde_bytes::{ByteBuf, Bytes};

    pub fn serialize<S>(value: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(bytes) => serializer.serialize_some(Bytes::new(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<ByteBuf>::deserialize(deserializer).map(|opt| opt.map(|buf| buf.into_vec()))
    }
}
