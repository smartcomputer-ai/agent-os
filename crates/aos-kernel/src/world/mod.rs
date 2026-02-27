use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use aos_air_types::{
    AirNode, DefCap, DefEffect, DefModule, DefPolicy, DefSchema, Manifest, Name, SecretDecl,
    SecretEntry, TypeExpr, TypePrimitive, builtins, schema_index::SchemaIndex,
    value_normalize::normalize_cbor_by_name,
};
use aos_cbor::{Hash, Hash as DigestHash, to_canonical_cbor};
use aos_effects::{EffectIntent, EffectKind, EffectReceipt};
use aos_store::Store;
use aos_wasm_abi::{ABI_VERSION, DomainEvent, PureInput, PureOutput, WorkflowInput, WorkflowOutput};
use getrandom::getrandom;
use serde::Serialize;
use serde_cbor;
use serde_cbor::Value as CborValue;

use crate::cap_enforcer::{CapEnforcerInvoker, PureCapEnforcer};
use crate::capability::{CapGrantResolution, CapabilityResolver};
use crate::cell_index::{CellIndex, CellMeta};
use crate::effects::{EffectManager, EffectParamPreprocessor};
use crate::error::KernelError;
use crate::event::{IngressStamp, KernelEvent, WorkflowEvent};
use crate::governance::{GovernanceManager, ManifestPatch, ProposalState};
use crate::governance_effects::GovernanceParamPreprocessor;
use crate::journal::fs::FsJournal;
use crate::journal::mem::MemJournal;
use crate::journal::{
    AppliedRecord, ApprovalDecisionRecord, ApprovedRecord, DomainEventRecord, EffectIntentRecord,
    EffectReceiptRecord, GovernanceRecord, IntentOriginRecord, Journal, JournalEntry, JournalKind,
    JournalRecord, JournalSeq, ManifestRecord, OwnedJournalEntry, ProposedRecord,
    ShadowReportRecord, SnapshotRecord, StreamFrameRecord,
};
use crate::manifest::{LoadedManifest, ManifestLoader};
use crate::pure::PureRegistry;
use crate::query::{Consistency, ReadMeta, StateRead, StateReader};
use crate::receipts::WorkflowEffectContext;
use crate::workflow::WorkflowRegistry;
use crate::schema_value::cbor_to_expr_value;
use crate::secret::{PlaceholderSecretResolver, SharedSecretResolver};
use crate::shadow::{
    DeltaKind, LedgerDelta, LedgerKind, ShadowConfig, ShadowExecutor, ShadowHarness, ShadowSummary,
};
use crate::snapshot::{
    EffectIntentSnapshot, KernelSnapshot, WorkflowReceiptSnapshot, WorkflowStateEntry,
    SnapshotRootCompleteness, WorkflowInflightIntentSnapshot, WorkflowInstanceSnapshot,
    WorkflowStatusSnapshot, receipts_to_vecdeque,
};
use std::sync::Mutex;

mod bootstrap;
mod event_flow;
pub(crate) mod governance_runtime;
mod manifest_runtime;
mod runtime;
mod query_api;
mod snapshot_replay;
#[cfg(test)]
pub(crate) mod test_support;

pub use crate::governance_utils::canonicalize_patch;

const RECENT_RECEIPT_CACHE: usize = 512;
const CELL_CACHE_SIZE: usize = 128;
const MONO_KEY: &[u8] = b"";
const ENTROPY_LEN: usize = 64;

#[derive(Debug)]
struct KernelClock {
    start: Instant,
    logical_offset_ns: AtomicU64,
}

impl KernelClock {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            logical_offset_ns: AtomicU64::new(0),
        }
    }

    fn now_wall_ns(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
    }

    fn logical_now_ns(&self) -> u64 {
        self.logical_offset_ns.load(Ordering::Relaxed) + self.start.elapsed().as_nanos() as u64
    }

    fn sync_logical_min(&self, target_ns: u64) {
        let current = self.logical_now_ns();
        if target_ns > current {
            self.logical_offset_ns
                .fetch_add(target_ns - current, Ordering::Relaxed);
        }
    }
}

#[derive(Clone, Default)]
pub struct KernelConfig {
    pub module_cache_dir: Option<PathBuf>,
    pub eager_module_load: bool,
    pub secret_resolver: Option<SharedSecretResolver>,
    pub allow_placeholder_secrets: bool,
}

impl fmt::Debug for KernelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KernelConfig")
            .field("module_cache_dir", &self.module_cache_dir)
            .field("eager_module_load", &self.eager_module_load)
            .field(
                "secret_resolver",
                &self.secret_resolver.as_ref().map(|_| "<resolver>"),
            )
            .field("allow_placeholder_secrets", &self.allow_placeholder_secrets)
            .finish()
    }
}

pub struct Kernel<S: Store> {
    store: Arc<S>,
    manifest: Manifest,
    manifest_hash: Hash,
    secrets: Vec<SecretDecl>,
    module_defs: HashMap<Name, aos_air_types::DefModule>,
    cap_defs: HashMap<Name, DefCap>,
    effect_defs: HashMap<Name, DefEffect>,
    policy_defs: HashMap<Name, DefPolicy>,
    schema_defs: HashMap<Name, DefSchema>,
    workflows: WorkflowRegistry<S>,
    pures: Arc<Mutex<PureRegistry<S>>>,
    router: HashMap<String, Vec<RouteBinding>>,
    schema_index: Arc<SchemaIndex>,
    workflow_schemas: Arc<HashMap<Name, WorkflowSchema>>,
    module_cap_bindings: HashMap<Name, HashMap<String, CapGrantResolution>>,
    pending_workflow_receipts: HashMap<[u8; 32], WorkflowEffectContext>,
    recent_receipts: VecDeque<[u8; 32]>,
    recent_receipt_index: HashSet<[u8; 32]>,
    workflow_queue: VecDeque<WorkflowEvent>,
    effect_manager: EffectManager,
    clock: KernelClock,
    workflow_state: HashMap<Name, WorkflowState>,
    workflow_instances: HashMap<String, WorkflowInstanceState>,
    workflow_index_roots: HashMap<Name, Hash>,
    snapshot_index: HashMap<JournalSeq, (Hash, Option<Hash>)>,
    journal: Box<dyn Journal>,
    suppress_journal: bool,
    replay_applying_domain_record: bool,
    replay_generated_domain_event_hashes: HashMap<String, u64>,
    governance: GovernanceManager,
    secret_resolver: Option<SharedSecretResolver>,
    allow_placeholder_secrets: bool,
    active_baseline: Option<SnapshotRecord>,
    last_snapshot_height: Option<JournalSeq>,
    last_snapshot_hash: Option<Hash>,
    pinned_roots: Vec<Hash>,
    workspace_roots: Vec<Hash>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowRuntimeStatus {
    Running,
    Waiting,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowInflightIntentMeta {
    pub origin_module_id: String,
    pub origin_instance_key: Option<Vec<u8>>,
    pub effect_kind: String,
    pub params_hash: Option<String>,
    pub emitted_at_seq: u64,
    pub last_stream_seq: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowInstanceState {
    pub state_bytes: Vec<u8>,
    pub inflight_intents: BTreeMap<[u8; 32], WorkflowInflightIntentMeta>,
    pub status: WorkflowRuntimeStatus,
    pub last_processed_event_seq: u64,
    pub module_version: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernelHeights {
    pub snapshot: Option<JournalSeq>,
    pub head: JournalSeq,
}

#[derive(Debug, Clone, Serialize)]
pub struct DefListing {
    pub kind: String,
    pub name: String,
    pub hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cap_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params_schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_steps: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_rules: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailIntent {
    pub seq: JournalSeq,
    pub record: EffectIntentRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailReceipt {
    pub seq: JournalSeq,
    pub record: EffectReceiptRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailEntry {
    pub seq: JournalSeq,
    pub kind: JournalKind,
    pub record: JournalRecord,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TailScan {
    pub from: JournalSeq,
    pub to: JournalSeq,
    pub entries: Vec<TailEntry>,
    pub intents: Vec<TailIntent>,
    pub receipts: Vec<TailReceipt>,
}

fn def_kind_allowed(kind: &str, filter: Option<&std::collections::HashSet<&str>>) -> bool {
    if let Some(set) = filter {
        set.contains(kind)
    } else {
        true
    }
}

fn normalize_def_kind(input: &str) -> Option<&'static str> {
    match input {
        "defschema" | "schema" => Some("defschema"),
        "defmodule" | "module" => Some("defmodule"),
        "defcap" | "cap" => Some("defcap"),
        "defeffect" | "effect" => Some("defeffect"),
        "defpolicy" | "policy" => Some("defpolicy"),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub(super) struct WorkflowSchema {
    pub event_schema_name: String,
    pub event_schema: TypeExpr,
    pub key_schema: Option<TypeExpr>,
}

#[derive(Clone, Debug)]
enum EventWrap {
    Identity,
    Variant { tag: String },
}

#[derive(Clone, Debug)]
struct RouteBinding {
    workflow: Name,
    key_field: Option<String>,
    route_event_schema: String,
    workflow_event_schema: String,
    wrap: EventWrap,
}

#[derive(Clone)]
struct WorkflowState {
    cell_cache: CellCache,
}

impl Default for WorkflowState {
    fn default() -> Self {
        Self {
            cell_cache: CellCache::new(CELL_CACHE_SIZE),
        }
    }
}

#[derive(Clone)]
struct CellEntry {
    state: Vec<u8>,
    state_hash: Hash,
    last_active_ns: u64,
}

#[derive(Clone)]
struct CellCache {
    capacity: usize,
    map: HashMap<Vec<u8>, CellEntry>,
    order: VecDeque<Vec<u8>>,
}

impl CellCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&mut self, key: &[u8]) -> Option<CellEntry> {
        let entry = self.map.get(key).cloned();
        if entry.is_some() {
            self.promote(key);
        }
        entry
    }

    fn get_ref(&self, key: &[u8]) -> Option<&CellEntry> {
        self.map.get(key)
    }

    fn insert(&mut self, key: Vec<u8>, entry: CellEntry) {
        if self.map.contains_key(&key) {
            self.map.insert(key.clone(), entry);
            self.promote(&key);
            return;
        }
        if self.capacity > 0 && self.map.len() >= self.capacity {
            if let Some(evicted) = self.order.pop_front() {
                self.map.remove(&evicted);
            }
        }
        self.order.push_back(key.clone());
        self.map.insert(key, entry);
    }

    fn remove(&mut self, key: &[u8]) {
        self.map.remove(key);
        self.order.retain(|k| k.as_slice() != key);
    }

    fn promote(&mut self, key: &[u8]) {
        self.order.retain(|k| k.as_slice() != key);
        self.order.push_back(key.to_vec());
    }
}

pub struct KernelBuilder<S: Store> {
    store: Arc<S>,
    journal: Box<dyn Journal>,
    config: KernelConfig,
}

impl<S: Store + 'static> KernelBuilder<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            journal: Box::new(MemJournal::new()),
            config: KernelConfig::default(),
        }
    }

    pub fn with_journal(mut self, journal: Box<dyn Journal>) -> Self {
        self.journal = journal;
        self
    }

    pub fn with_fs_journal(
        mut self,
        root: impl AsRef<std::path::Path>,
    ) -> Result<Self, KernelError> {
        let journal = FsJournal::open(root)?;
        self.journal = Box::new(journal);
        Ok(self)
    }

    pub fn with_module_cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.config.module_cache_dir = Some(dir.into());
        self
    }

    pub fn with_eager_module_load(mut self, enable: bool) -> Self {
        self.config.eager_module_load = enable;
        self
    }

    pub fn with_secret_resolver(mut self, resolver: SharedSecretResolver) -> Self {
        self.config.secret_resolver = Some(resolver);
        self
    }

    pub fn allow_placeholder_secrets(mut self, enable: bool) -> Self {
        self.config.allow_placeholder_secrets = enable;
        self
    }

    pub fn from_manifest_path(
        self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<Kernel<S>, KernelError> {
        let loaded = ManifestLoader::load_from_path(&*self.store, path)?;
        Kernel::from_loaded_manifest_with_config(self.store, loaded, self.journal, self.config)
    }

    pub fn from_loaded_manifest(self, loaded: LoadedManifest) -> Result<Kernel<S>, KernelError> {
        Kernel::from_loaded_manifest_with_config(self.store, loaded, self.journal, self.config)
    }
}

impl<S: Store + 'static> Kernel<S> {
    fn ensure_cell_index_root(&mut self, workflow: &Name) -> Result<Hash, KernelError> {
        if let Some(root) = self.workflow_index_roots.get(workflow) {
            return Ok(*root);
        }
        let index = CellIndex::new(self.store.as_ref());
        let root = index
            .empty()
            .map_err(|err| KernelError::SnapshotUnavailable(err.to_string()))?;
        self.workflow_index_roots.insert(workflow.clone(), root);
        Ok(root)
    }

    pub fn invoke_pure(
        &mut self,
        name: &str,
        input: &PureInput,
    ) -> Result<PureOutput, KernelError> {
        let module_def = self
            .module_defs
            .get(name)
            .ok_or_else(|| KernelError::PureNotFound(name.to_string()))?;
        if module_def.module_kind != aos_air_types::ModuleKind::Pure {
            return Err(KernelError::Manifest(format!(
                "module '{name}' is not a pure module"
            )));
        }
        let wants_context = module_def
            .abi
            .pure
            .as_ref()
            .and_then(|abi| abi.context.as_ref())
            .is_some();
        if wants_context && input.ctx.is_none() {
            return Err(KernelError::Manifest(format!(
                "pure module '{name}' requires call context"
            )));
        }
        let input = if wants_context {
            input.clone()
        } else {
            PureInput {
                version: input.version,
                input: input.input.clone(),
                ctx: None,
            }
        };
        let mut pures = self
            .pures
            .lock()
            .map_err(|_| KernelError::Manifest("pure registry lock poisoned".into()))?;
        pures.ensure_loaded(name, module_def)?;
        pures.invoke(name, &input)
    }

    /// Access underlying store (Arc clone).
    pub fn store(&self) -> Arc<S> {
        self.store.clone()
    }

    fn restore_effect_intent(&mut self, record: EffectIntentRecord) -> Result<(), KernelError> {
        let effect_kind = record.kind.clone();
        let params_cbor = record.params_cbor.clone();
        match record.origin {
            IntentOriginRecord::Workflow {
                name,
                instance_key,
                emitted_at_seq,
            } => {
                if self
                    .pending_workflow_receipts
                    .contains_key(&record.intent_hash)
                {
                    return Ok(());
                }
                self.pending_workflow_receipts
                    .entry(record.intent_hash)
                    .or_insert_with(|| {
                        WorkflowEffectContext::new(
                            name.clone(),
                            instance_key.clone(),
                            effect_kind.clone(),
                            params_cbor.clone(),
                            record.intent_hash,
                            emitted_at_seq.unwrap_or_default(),
                            None,
                        )
                    });
                self.record_workflow_inflight_intent(
                    &name,
                    instance_key.as_deref(),
                    record.intent_hash,
                    &effect_kind,
                    &params_cbor,
                    emitted_at_seq.unwrap_or_default(),
                );
            }
            IntentOriginRecord::Plan { name: _, plan_id } => {
                log::warn!(
                    "ignoring replayed plan-origin intent {} for retired plan runtime (plan_id={})",
                    format_intent_hash(&record.intent_hash),
                    plan_id
                );
            }
        }
        Ok(())
    }

    pub(crate) fn record_workflow_state_transition(
        &mut self,
        module_id: &str,
        instance_key: Option<&[u8]>,
        state_bytes: Option<&[u8]>,
        last_processed_event_seq: u64,
        module_version: Option<String>,
    ) {
        let instance_id = workflow_instance_id(module_id, instance_key);
        let entry = self
            .workflow_instances
            .entry(instance_id)
            .or_insert_with(|| WorkflowInstanceState {
                state_bytes: Vec::new(),
                inflight_intents: BTreeMap::new(),
                status: WorkflowRuntimeStatus::Running,
                last_processed_event_seq,
                module_version: module_version.clone(),
            });
        if let Some(bytes) = state_bytes {
            entry.state_bytes = bytes.to_vec();
        } else {
            entry.state_bytes.clear();
            entry.status = WorkflowRuntimeStatus::Completed;
        }
        entry.last_processed_event_seq = last_processed_event_seq;
        if module_version.is_some() {
            entry.module_version = module_version;
        }
        refresh_workflow_status(entry);
    }

    pub(crate) fn record_workflow_inflight_intent(
        &mut self,
        module_id: &str,
        instance_key: Option<&[u8]>,
        intent_hash: [u8; 32],
        effect_kind: &str,
        params_cbor: &[u8],
        emitted_at_seq: u64,
    ) {
        let instance_id = workflow_instance_id(module_id, instance_key);
        let entry = self
            .workflow_instances
            .entry(instance_id)
            .or_insert_with(|| WorkflowInstanceState {
                state_bytes: Vec::new(),
                inflight_intents: BTreeMap::new(),
                status: WorkflowRuntimeStatus::Running,
                last_processed_event_seq: emitted_at_seq,
                module_version: None,
            });
        entry.inflight_intents.insert(
            intent_hash,
            WorkflowInflightIntentMeta {
                origin_module_id: module_id.to_string(),
                origin_instance_key: instance_key.map(|k| k.to_vec()),
                effect_kind: effect_kind.to_string(),
                params_hash: Some(Hash::of_bytes(params_cbor).to_hex()),
                emitted_at_seq,
                last_stream_seq: 0,
            },
        );
        entry.last_processed_event_seq = emitted_at_seq;
        refresh_workflow_status(entry);
    }

    pub(crate) fn mark_workflow_receipt_settled(
        &mut self,
        module_id: &str,
        instance_key: Option<&[u8]>,
        intent_hash: [u8; 32],
    ) {
        let instance_id = workflow_instance_id(module_id, instance_key);
        let Some(entry) = self.workflow_instances.get_mut(&instance_id) else {
            return;
        };
        entry.inflight_intents.remove(&intent_hash);
        refresh_workflow_status(entry);
    }

    pub(crate) fn workflow_stream_cursor(
        &self,
        module_id: &str,
        instance_key: Option<&[u8]>,
        intent_hash: [u8; 32],
    ) -> Option<u64> {
        let instance_id = workflow_instance_id(module_id, instance_key);
        self.workflow_instances
            .get(&instance_id)
            .and_then(|entry| entry.inflight_intents.get(&intent_hash))
            .map(|meta| meta.last_stream_seq)
    }

    pub(crate) fn advance_workflow_stream_cursor(
        &mut self,
        module_id: &str,
        instance_key: Option<&[u8]>,
        intent_hash: [u8; 32],
        seq: u64,
    ) -> bool {
        let instance_id = workflow_instance_id(module_id, instance_key);
        let Some(entry) = self.workflow_instances.get_mut(&instance_id) else {
            return false;
        };
        let Some(meta) = entry.inflight_intents.get_mut(&intent_hash) else {
            return false;
        };
        meta.last_stream_seq = seq;
        true
    }

    pub fn workflow_state(&self, workflow: &str) -> Option<Vec<u8>> {
        self.workflow_state_bytes(workflow, None).ok().flatten()
    }

    /// Fetch workflow state bytes via the cell index (non-keyed workflows use the sentinel key).
    pub fn workflow_state_bytes(
        &self,
        workflow: &str,
        key: Option<&[u8]>,
    ) -> Result<Option<Vec<u8>>, KernelError> {
        let key = key.unwrap_or(MONO_KEY);
        let Some(root) = self.workflow_index_roots.get(workflow) else {
            return Ok(None);
        };
        let index = CellIndex::new(self.store.as_ref());
        let meta = index.get(*root, Hash::of_bytes(key).as_bytes())?;
        if let Some(meta) = meta {
            let state_hash = Hash::from_bytes(&meta.state_hash)
                .unwrap_or_else(|_| Hash::of_bytes(&meta.state_hash));
            let state = self.store.get_blob(state_hash)?;
            Ok(Some(state))
        } else {
            Ok(None)
        }
    }

    /// Returns typed hash references reachable from workflow state by traversing the workflow's
    /// declared state schema. Hash-like text/bytes in opaque fields are ignored.
    pub fn workflow_state_typed_hash_refs(
        &self,
        workflow: &str,
        key: Option<&[u8]>,
    ) -> Result<Vec<Hash>, KernelError> {
        let Some(state_bytes) = self.workflow_state_bytes(workflow, key)? else {
            return Ok(Vec::new());
        };
        let module = self
            .module_defs
            .get(workflow)
            .ok_or_else(|| KernelError::WorkflowNotFound(workflow.to_string()))?;
        let workflow_abi =
            module.abi.workflow.as_ref().ok_or_else(|| {
                KernelError::Manifest(format!("module '{workflow}' is not a workflow"))
            })?;
        let schema = self
            .schema_index
            .get(workflow_abi.state.as_str())
            .ok_or_else(|| {
                KernelError::Manifest(format!(
                    "state schema '{}' not found for workflow '{workflow}'",
                    workflow_abi.state
                ))
            })?;
        let value: CborValue = serde_cbor::from_slice(&state_bytes)
            .map_err(|err| KernelError::SnapshotDecode(err.to_string()))?;
        let mut refs = Vec::new();
        collect_typed_hash_refs(&value, schema, &self.schema_index, &mut refs)?;
        refs.sort();
        refs.dedup();
        Ok(refs)
    }

    pub(crate) fn canonical_key_bytes(
        &self,
        schema_name: &str,
        value: &str,
    ) -> Result<Vec<u8>, KernelError> {
        let cbor = serde_cbor::to_vec(&value).map_err(|e| KernelError::Manifest(e.to_string()))?;
        let normalized = normalize_cbor_by_name(&self.schema_index, schema_name, &cbor)
            .map_err(|err| KernelError::Manifest(err.to_string()))?;
        Ok(normalized.bytes)
    }

    pub(crate) fn read_meta(&self) -> ReadMeta {
        let (active_baseline_height, active_baseline_receipt_horizon_height) = self
            .active_baseline
            .as_ref()
            .map(|b| (Some(b.height), b.receipt_horizon_height))
            .unwrap_or((None, None));
        ReadMeta {
            journal_height: self.journal.next_seq(),
            snapshot_hash: self.last_snapshot_hash,
            manifest_hash: self.manifest_hash,
            active_baseline_height,
            active_baseline_receipt_horizon_height,
        }
    }

    /// Current manifest hash (canonical CBOR of manifest node).
    pub fn manifest_hash(&self) -> Hash {
        self.manifest_hash
    }

    /// Look up a def node by name from the active manifest.
    pub fn get_def(&self, name: &str) -> Option<AirNode> {
        if let Some(def) = self.schema_defs.get(name) {
            return Some(AirNode::Defschema(def.clone()));
        }
        if let Some(def) = self.module_defs.get(name) {
            return Some(AirNode::Defmodule(def.clone()));
        }
        if let Some(def) = self.cap_defs.get(name) {
            return Some(AirNode::Defcap(def.clone()));
        }
        if let Some(def) = self.policy_defs.get(name) {
            return Some(AirNode::Defpolicy(def.clone()));
        }
        self.effect_defs
            .get(name)
            .map(|def| AirNode::Defeffect(def.clone()))
    }

    /// Hash of the most recent snapshot blob, if any.
    pub fn snapshot_hash(&self) -> Option<Hash> {
        self.last_snapshot_hash
    }

    /// List defs from the active manifest with optional kind/prefix filters.
    pub fn list_defs(&self, kinds: Option<&[String]>, prefix: Option<&str>) -> Vec<DefListing> {
        let prefix = prefix.unwrap_or("");
        let kind_filter: Option<std::collections::HashSet<&str>> = kinds.map(|ks| {
            ks.iter()
                .filter_map(|k| normalize_def_kind(k.as_str()))
                .collect::<std::collections::HashSet<&str>>()
        });

        let mut entries = Vec::new();
        let hash_def =
            |node: AirNode| -> String { Hash::of_cbor(&node).expect("hash def").to_hex() };

        fn push_if<F>(
            entries: &mut Vec<DefListing>,
            kind: &str,
            name: &str,
            build: F,
            filter: &Option<std::collections::HashSet<&str>>,
            prefix: &str,
        ) where
            F: FnOnce() -> DefListing,
        {
            if !name.starts_with(prefix) {
                return;
            }
            if !def_kind_allowed(kind, filter.as_ref()) {
                return;
            }
            entries.push(build());
        }

        for (name, def) in self.schema_defs.iter() {
            push_if(
                &mut entries,
                "defschema",
                name.as_str(),
                || DefListing {
                    kind: "defschema".into(),
                    name: name.clone(),
                    hash: hash_def(AirNode::Defschema(def.clone())),
                    cap_type: None,
                    params_schema: None,
                    receipt_schema: None,
                    plan_steps: None,
                    policy_rules: None,
                },
                &kind_filter,
                prefix,
            );
        }

        for (name, def) in self.module_defs.iter() {
            push_if(
                &mut entries,
                "defmodule",
                name.as_str(),
                || DefListing {
                    kind: "defmodule".into(),
                    name: name.clone(),
                    hash: hash_def(AirNode::Defmodule(def.clone())),
                    cap_type: None,
                    params_schema: None,
                    receipt_schema: None,
                    plan_steps: None,
                    policy_rules: None,
                },
                &kind_filter,
                prefix,
            );
        }

        for (name, def) in self.cap_defs.iter() {
            push_if(
                &mut entries,
                "defcap",
                name.as_str(),
                || DefListing {
                    kind: "defcap".into(),
                    name: name.clone(),
                    hash: hash_def(AirNode::Defcap(def.clone())),
                    cap_type: Some(def.cap_type.as_str().to_string()),
                    params_schema: None,
                    receipt_schema: None,
                    plan_steps: None,
                    policy_rules: None,
                },
                &kind_filter,
                prefix,
            );
        }

        for (name, def) in self.effect_defs.iter() {
            push_if(
                &mut entries,
                "defeffect",
                name.as_str(),
                || DefListing {
                    kind: "defeffect".into(),
                    name: name.clone(),
                    hash: hash_def(AirNode::Defeffect(def.clone())),
                    cap_type: Some(def.cap_type.as_str().to_string()),
                    params_schema: Some(def.params_schema.as_str().to_string()),
                    receipt_schema: Some(def.receipt_schema.as_str().to_string()),
                    plan_steps: None,
                    policy_rules: None,
                },
                &kind_filter,
                prefix,
            );
        }

        for (name, def) in self.policy_defs.iter() {
            push_if(
                &mut entries,
                "defpolicy",
                name.as_str(),
                || DefListing {
                    kind: "defpolicy".into(),
                    name: name.clone(),
                    hash: hash_def(AirNode::Defpolicy(def.clone())),
                    cap_type: None,
                    params_schema: None,
                    receipt_schema: None,
                    plan_steps: None,
                    policy_rules: Some(def.rules.len()),
                },
                &kind_filter,
                prefix,
            );
        }

        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
    }

    /// List all cells for a workflow using the persisted CellIndex.
    ///
    /// Returns an empty Vec if the workflow has no cells yet.
    pub fn list_cells(&self, workflow: &str) -> Result<Vec<CellMeta>, KernelError> {
        let Some(root) = self.workflow_index_roots.get(workflow) else {
            return Ok(Vec::new());
        };
        let index = CellIndex::new(self.store.as_ref());
        let mut metas = Vec::new();
        for meta in index.iter(*root) {
            metas.push(meta?);
        }
        Ok(metas)
    }

    pub fn heights(&self) -> KernelHeights {
        KernelHeights {
            snapshot: self.last_snapshot_height,
            head: self.journal.next_seq(),
        }
    }

    pub fn logical_time_now_ns(&self) -> u64 {
        self.clock.logical_now_ns()
    }

    pub fn journal_head(&self) -> JournalSeq {
        self.journal.next_seq()
    }

    pub fn queued_effects_snapshot(&self) -> Vec<EffectIntentSnapshot> {
        self.effect_manager
            .queued()
            .iter()
            .map(EffectIntentSnapshot::from_intent)
            .collect()
    }

    /// Expose workflow index root hash (if present) for keyed workflows; useful for diagnostics/tests.
    pub fn workflow_index_root(&self, workflow: &str) -> Option<Hash> {
        self.workflow_index_roots.get(workflow).copied()
    }

    pub fn pending_workflow_receipts_snapshot(&self) -> Vec<WorkflowReceiptSnapshot> {
        self.pending_workflow_receipts
            .iter()
            .map(|(hash, ctx)| WorkflowReceiptSnapshot::from_context(*hash, ctx))
            .collect()
    }

    pub fn workflow_instances_snapshot(&self) -> Vec<WorkflowInstanceSnapshot> {
        self.workflow_instances
            .iter()
            .map(|(instance_id, state)| WorkflowInstanceSnapshot {
                instance_id: instance_id.clone(),
                state_bytes: state.state_bytes.clone(),
                inflight_intents: state
                    .inflight_intents
                    .iter()
                    .map(|(intent_id, meta)| WorkflowInflightIntentSnapshot {
                        intent_id: *intent_id,
                        origin_module_id: meta.origin_module_id.clone(),
                        origin_instance_key: meta.origin_instance_key.clone(),
                        effect_kind: meta.effect_kind.clone(),
                        params_hash: meta.params_hash.clone(),
                        emitted_at_seq: meta.emitted_at_seq,
                        last_stream_seq: meta.last_stream_seq,
                    })
                    .collect(),
                status: match state.status {
                    WorkflowRuntimeStatus::Running => WorkflowStatusSnapshot::Running,
                    WorkflowRuntimeStatus::Waiting => WorkflowStatusSnapshot::Waiting,
                    WorkflowRuntimeStatus::Completed => WorkflowStatusSnapshot::Completed,
                    WorkflowRuntimeStatus::Failed => WorkflowStatusSnapshot::Failed,
                },
                last_processed_event_seq: state.last_processed_event_seq,
                module_version: state.module_version.clone(),
            })
            .collect()
    }

    pub fn dump_journal(&self) -> Result<Vec<OwnedJournalEntry>, KernelError> {
        Ok(self.journal.load_from(0)?)
    }

    pub fn governance(&self) -> &GovernanceManager {
        &self.governance
    }

    fn sample_ingress(&mut self, journal_height: u64) -> Result<IngressStamp, KernelError> {
        let now_ns = self.clock.now_wall_ns();
        let sampled_logical = self.clock.logical_now_ns();
        self.effect_manager.update_logical_now_ns(sampled_logical);
        let logical_now_ns = self.effect_manager.logical_now_ns();
        self.clock.sync_logical_min(logical_now_ns);

        let mut entropy = vec![0u8; ENTROPY_LEN];
        getrandom(&mut entropy).map_err(|err| KernelError::Entropy(err.to_string()))?;

        Ok(IngressStamp {
            now_ns,
            logical_now_ns,
            entropy,
            journal_height,
            manifest_hash: self.manifest_hash.to_hex(),
        })
    }

    fn validate_entropy(entropy: &[u8]) -> Result<(), KernelError> {
        if entropy.len() != ENTROPY_LEN {
            return Err(KernelError::Entropy(format!(
                "entropy length must be {ENTROPY_LEN} bytes (got {})",
                entropy.len()
            )));
        }
        Ok(())
    }

    fn sync_logical_from_record(&mut self, logical_now_ns: u64) {
        self.effect_manager.update_logical_now_ns(logical_now_ns);
        let logical_now_ns = self.effect_manager.logical_now_ns();
        self.clock.sync_logical_min(logical_now_ns);
    }

    fn mark_replay_generated_domain_event(
        &mut self,
        event: &DomainEvent,
    ) -> Result<(), KernelError> {
        if !self.suppress_journal || self.replay_applying_domain_record {
            return Ok(());
        }
        let hash = Hash::of_cbor(event)
            .map_err(|err| KernelError::Journal(err.to_string()))?
            .to_hex();
        let count = self
            .replay_generated_domain_event_hashes
            .entry(hash)
            .or_insert(0);
        *count += 1;
        Ok(())
    }

    fn consume_replay_generated_domain_event(&mut self, event_hash: &str) -> bool {
        let Some(count) = self
            .replay_generated_domain_event_hashes
            .get_mut(event_hash)
        else {
            return false;
        };
        if *count <= 1 {
            self.replay_generated_domain_event_hashes.remove(event_hash);
        } else {
            *count -= 1;
        }
        true
    }

    fn append_record(&mut self, record: JournalRecord) -> Result<(), KernelError> {
        if self.suppress_journal {
            return Ok(());
        }
        let bytes =
            serde_cbor::to_vec(&record).map_err(|err| KernelError::Journal(err.to_string()))?;
        self.journal
            .append(JournalEntry::new(record.kind(), &bytes))?;
        Ok(())
    }

    fn record_manifest(&mut self) -> Result<(), KernelError> {
        self.append_record(JournalRecord::Manifest(ManifestRecord {
            manifest_hash: self.manifest_hash.to_hex(),
        }))
    }

    fn record_domain_event(
        &mut self,
        event: &DomainEvent,
        stamp: &IngressStamp,
    ) -> Result<(), KernelError> {
        if self.suppress_journal {
            return Ok(());
        }
        let event_hash = Hash::of_cbor(event)
            .map_err(|err| KernelError::Journal(err.to_string()))?
            .to_hex();
        let record = JournalRecord::DomainEvent(DomainEventRecord {
            schema: event.schema.clone(),
            value: event.value.clone(),
            key: event.key.clone(),
            now_ns: stamp.now_ns,
            logical_now_ns: stamp.logical_now_ns,
            journal_height: stamp.journal_height,
            entropy: stamp.entropy.clone(),
            event_hash,
            manifest_hash: stamp.manifest_hash.clone(),
        });
        self.append_record(record)
    }

    fn record_effect_intent(
        &mut self,
        intent: &EffectIntent,
        origin: IntentOriginRecord,
    ) -> Result<(), KernelError> {
        let record = JournalRecord::EffectIntent(EffectIntentRecord {
            intent_hash: intent.intent_hash,
            kind: intent.kind.as_str().to_string(),
            cap_name: intent.cap_name.clone(),
            params_cbor: intent.params_cbor.clone(),
            idempotency_key: intent.idempotency_key,
            origin,
        });
        self.append_record(record)
    }

    fn record_effect_receipt(
        &mut self,
        receipt: &EffectReceipt,
        stamp: &IngressStamp,
    ) -> Result<(), KernelError> {
        if self.suppress_journal {
            return Ok(());
        }
        let record = JournalRecord::EffectReceipt(EffectReceiptRecord {
            intent_hash: receipt.intent_hash,
            adapter_id: receipt.adapter_id.clone(),
            status: receipt.status.clone(),
            payload_cbor: receipt.payload_cbor.clone(),
            cost_cents: receipt.cost_cents,
            signature: receipt.signature.clone(),
            now_ns: stamp.now_ns,
            logical_now_ns: stamp.logical_now_ns,
            journal_height: stamp.journal_height,
            entropy: stamp.entropy.clone(),
            manifest_hash: stamp.manifest_hash.clone(),
        });
        self.append_record(record)
    }

    fn record_stream_frame(
        &mut self,
        frame: &aos_effects::EffectStreamFrame,
        stamp: &IngressStamp,
    ) -> Result<(), KernelError> {
        if self.suppress_journal {
            return Ok(());
        }
        let record = JournalRecord::StreamFrame(StreamFrameRecord {
            intent_hash: frame.intent_hash,
            adapter_id: frame.adapter_id.clone(),
            origin_module_id: frame.origin_module_id.clone(),
            origin_instance_key: frame.origin_instance_key.clone(),
            effect_kind: frame.effect_kind.clone(),
            emitted_at_seq: frame.emitted_at_seq,
            seq: frame.seq,
            frame_kind: frame.kind.clone(),
            payload_cbor: frame.payload_cbor.clone(),
            payload_ref: frame.payload_ref.clone(),
            signature: frame.signature.clone(),
            now_ns: stamp.now_ns,
            logical_now_ns: stamp.logical_now_ns,
            journal_height: stamp.journal_height,
            entropy: stamp.entropy.clone(),
            manifest_hash: stamp.manifest_hash.clone(),
        });
        self.append_record(record)
    }

    fn record_decisions(&mut self) -> Result<(), KernelError> {
        let records = self.effect_manager.drain_cap_decisions();
        for record in records {
            self.append_record(JournalRecord::CapDecision(record))?;
        }
        let policy_records = self.effect_manager.drain_policy_decisions();
        for record in policy_records {
            self.append_record(JournalRecord::PolicyDecision(record))?;
        }
        Ok(())
    }
}

fn collect_typed_hash_refs(
    value: &CborValue,
    schema: &TypeExpr,
    schemas: &SchemaIndex,
    out: &mut Vec<Hash>,
) -> Result<(), KernelError> {
    match schema {
        TypeExpr::Primitive(TypePrimitive::Hash(_)) => {
            if let CborValue::Text(text) = value {
                let hash = Hash::from_hex_str(text)
                    .map_err(|err| KernelError::Manifest(format!("invalid hash ref: {err}")))?;
                out.push(hash);
            }
        }
        TypeExpr::Primitive(_) => {}
        TypeExpr::Record(record) => {
            let CborValue::Map(map) = value else {
                return Err(KernelError::Manifest("expected record map".into()));
            };
            for (field, ty) in &record.record {
                let resolved = resolve_type_ref(ty, schemas)?;
                let field_value = map
                    .get(&CborValue::Text(field.clone()))
                    .unwrap_or(&CborValue::Null);
                collect_typed_hash_refs(field_value, resolved, schemas, out)?;
            }
        }
        TypeExpr::Variant(variant) => {
            let CborValue::Map(map) = value else {
                return Err(KernelError::Manifest("expected variant map".into()));
            };
            let tag = map
                .get(&CborValue::Text("$tag".into()))
                .and_then(|v| match v {
                    CborValue::Text(text) => Some(text),
                    _ => None,
                })
                .ok_or_else(|| KernelError::Manifest("variant missing $tag".into()))?;
            let Some(ty) = variant.variant.get(tag) else {
                return Err(KernelError::Manifest(format!(
                    "unknown variant tag '{tag}'"
                )));
            };
            let resolved = resolve_type_ref(ty, schemas)?;
            if let Some(inner) = map.get(&CborValue::Text("$value".into())) {
                collect_typed_hash_refs(inner, resolved, schemas, out)?;
            }
        }
        TypeExpr::List(list) => {
            let CborValue::Array(items) = value else {
                return Err(KernelError::Manifest("expected array".into()));
            };
            let resolved = resolve_type_ref(&list.list, schemas)?;
            for item in items {
                collect_typed_hash_refs(item, resolved, schemas, out)?;
            }
        }
        TypeExpr::Set(set) => {
            let CborValue::Array(items) = value else {
                return Err(KernelError::Manifest("expected array".into()));
            };
            let resolved = resolve_type_ref(&set.set, schemas)?;
            for item in items {
                collect_typed_hash_refs(item, resolved, schemas, out)?;
            }
        }
        TypeExpr::Map(map_ty) => {
            let CborValue::Map(map) = value else {
                return Err(KernelError::Manifest("expected map".into()));
            };
            for (k, v) in map {
                if matches!(map_ty.map.key, aos_air_types::TypeMapKey::Hash(_)) {
                    if let CborValue::Text(text) = k {
                        let hash = Hash::from_hex_str(text).map_err(|err| {
                            KernelError::Manifest(format!("invalid hash map key: {err}"))
                        })?;
                        out.push(hash);
                    }
                }
                let resolved = resolve_type_ref(&map_ty.map.value, schemas)?;
                collect_typed_hash_refs(v, resolved, schemas, out)?;
            }
        }
        TypeExpr::Option(opt) => {
            if !matches!(value, CborValue::Null) {
                let resolved = resolve_type_ref(&opt.option, schemas)?;
                collect_typed_hash_refs(value, resolved, schemas, out)?;
            }
        }
        TypeExpr::Ref(reference) => {
            let resolved = schemas.get(reference.reference.as_str()).ok_or_else(|| {
                KernelError::Manifest(format!("schema '{}' not found", reference.reference))
            })?;
            collect_typed_hash_refs(value, resolved, schemas, out)?;
        }
    }
    Ok(())
}

fn resolve_type_ref<'a>(
    schema: &'a TypeExpr,
    schemas: &'a SchemaIndex,
) -> Result<&'a TypeExpr, KernelError> {
    match schema {
        TypeExpr::Ref(reference) => schemas.get(reference.reference.as_str()).ok_or_else(|| {
            KernelError::Manifest(format!("schema '{}' not found", reference.reference))
        }),
        other => Ok(other),
    }
}

fn select_secret_resolver(
    has_secrets: bool,
    config: &KernelConfig,
) -> Result<Option<SharedSecretResolver>, KernelError> {
    ensure_secret_resolver(
        has_secrets,
        config.secret_resolver.clone(),
        config.allow_placeholder_secrets,
    )
}

fn ensure_secret_resolver(
    has_secrets: bool,
    provided: Option<SharedSecretResolver>,
    allow_placeholder: bool,
) -> Result<Option<SharedSecretResolver>, KernelError> {
    if !has_secrets {
        return Ok(None);
    }
    if let Some(resolver) = provided {
        return Ok(Some(resolver));
    }
    if allow_placeholder {
        return Ok(Some(Arc::new(PlaceholderSecretResolver)));
    }
    Err(KernelError::SecretResolverMissing)
}

fn format_intent_hash(hash: &[u8; 32]) -> String {
    DigestHash::from_bytes(hash)
        .map(|h| h.to_hex())
        .unwrap_or_else(|_| format!("{:?}", hash))
}

fn workflow_instance_id(module_id: &str, key: Option<&[u8]>) -> String {
    let key_hex = key.map(hex::encode).unwrap_or_default();
    format!("{module_id}::{key_hex}")
}

fn refresh_workflow_status(state: &mut WorkflowInstanceState) {
    if !state.inflight_intents.is_empty() {
        state.status = WorkflowRuntimeStatus::Waiting;
        return;
    }
    if matches!(
        state.status,
        WorkflowRuntimeStatus::Completed | WorkflowRuntimeStatus::Failed
    ) {
        return;
    }
    state.status = WorkflowRuntimeStatus::Running;
}
