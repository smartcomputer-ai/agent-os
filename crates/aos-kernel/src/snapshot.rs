use std::collections::{HashSet, VecDeque};

use aos_effects::{EffectIntent, EffectKind as RuntimeEffectKind};
use serde::{Deserialize, Serialize};
use serde_bytes;

use crate::journal::JournalSeq;
use crate::receipts::ReducerEffectContext;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelSnapshot {
    reducer_state: Vec<ReducerStateEntry>,
    reducer_index_roots: Vec<(String, [u8; 32])>,
    recent_receipts: Vec<[u8; 32]>,
    queued_effects: Vec<EffectIntentSnapshot>,
    pending_reducer_receipts: Vec<ReducerReceiptSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    workflow_instances: Vec<WorkflowInstanceSnapshot>,
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
        queued_effects: Vec<EffectIntentSnapshot>,
        pending_reducer_receipts: Vec<ReducerReceiptSnapshot>,
        workflow_instances: Vec<WorkflowInstanceSnapshot>,
        logical_now_ns: u64,
        manifest_hash: Option<[u8; 32]>,
    ) -> Self {
        Self {
            reducer_state,
            reducer_index_roots: Vec::new(),
            recent_receipts,
            queued_effects,
            pending_reducer_receipts,
            workflow_instances,
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

    pub fn queued_effects(&self) -> &[EffectIntentSnapshot] {
        &self.queued_effects
    }

    pub fn pending_reducer_receipts(&self) -> &[ReducerReceiptSnapshot] {
        &self.pending_reducer_receipts
    }

    pub fn workflow_instances(&self) -> &[WorkflowInstanceSnapshot] {
        &self.workflow_instances
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
pub struct ReducerReceiptSnapshot {
    pub intent_hash: [u8; 32],
    pub origin_module_id: String,
    pub effect_kind: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub origin_instance_key: Option<Vec<u8>>,
    #[serde(with = "serde_bytes")]
    pub params_cbor: Vec<u8>,
    #[serde(default)]
    pub emitted_at_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorkflowStatusSnapshot {
    Running,
    Waiting,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowInflightIntentSnapshot {
    pub intent_id: [u8; 32],
    pub origin_module_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub origin_instance_key: Option<Vec<u8>>,
    pub effect_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_hash: Option<String>,
    pub emitted_at_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowInstanceSnapshot {
    pub instance_id: String,
    #[serde(with = "serde_bytes")]
    pub state_bytes: Vec<u8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inflight_intents: Vec<WorkflowInflightIntentSnapshot>,
    pub status: WorkflowStatusSnapshot,
    pub last_processed_event_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_version: Option<String>,
}

impl ReducerReceiptSnapshot {
    pub fn from_context(intent_hash: [u8; 32], ctx: &ReducerEffectContext) -> Self {
        Self {
            intent_hash,
            origin_module_id: ctx.origin_module_id.clone(),
            effect_kind: ctx.effect_kind.clone(),
            origin_instance_key: ctx.origin_instance_key.clone(),
            params_cbor: ctx.params_cbor.clone(),
            emitted_at_seq: ctx.emitted_at_seq,
            module_version: ctx.module_version.clone(),
        }
    }

    pub fn into_context(self) -> ReducerEffectContext {
        ReducerEffectContext::new(
            self.origin_module_id,
            self.origin_instance_key,
            self.effect_kind,
            self.params_cbor,
            self.intent_hash,
            self.emitted_at_seq,
            self.module_version,
        )
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_roundtrip_preserves_workflow_instances() {
        let snapshot = KernelSnapshot::new(
            7,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![WorkflowInstanceSnapshot {
                instance_id: "com.acme/Workflow@1::".into(),
                state_bytes: vec![1, 2, 3],
                inflight_intents: vec![WorkflowInflightIntentSnapshot {
                    intent_id: [9u8; 32],
                    origin_module_id: "com.acme/Workflow@1".into(),
                    origin_instance_key: None,
                    effect_kind: "http.request".into(),
                    params_hash: None,
                    emitted_at_seq: 7,
                }],
                status: WorkflowStatusSnapshot::Waiting,
                last_processed_event_seq: 7,
                module_version: Some("sha256:abc".into()),
            }],
            123,
            None,
        );

        let bytes = serde_cbor::to_vec(&snapshot).expect("encode snapshot");
        let decoded: KernelSnapshot = serde_cbor::from_slice(&bytes).expect("decode snapshot");

        assert_eq!(decoded.workflow_instances().len(), 1);
        assert_eq!(decoded.workflow_instances()[0].instance_id, "com.acme/Workflow@1::");
        assert_eq!(decoded.workflow_instances()[0].inflight_intents.len(), 1);
    }
}
