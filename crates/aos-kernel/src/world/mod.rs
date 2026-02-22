use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use aos_air_exec::{Value as ExprValue, ValueKey};
use aos_air_types::{
    AirNode, DefCap, DefEffect, DefModule, DefPlan, DefPolicy, DefSchema, Expr, ExprOrValue,
    HashRef, Manifest, Name, NamedRef, PlanStepKind, SecretDecl, SecretEntry, TypeExpr,
    TypePrimitive, builtins,
    catalog::EffectCatalog,
    plan_literals::{SchemaIndex, normalize_plan_literals},
    value_normalize::{normalize_cbor_by_name, normalize_value_with_schema},
};
use aos_cbor::{Hash, Hash as DigestHash, to_canonical_cbor};
use aos_effects::{EffectIntent, EffectKind, EffectReceipt};
use aos_store::Store;
use aos_wasm_abi::{ABI_VERSION, DomainEvent, PureInput, PureOutput, ReducerInput, ReducerOutput};
use getrandom::getrandom;
use serde::Serialize;
use serde_cbor;
use serde_cbor::Value as CborValue;

use crate::cap_enforcer::{CapEnforcerInvoker, PureCapEnforcer};
use crate::capability::{CapGrantResolution, CapabilityResolver};
use crate::cell_index::{CellIndex, CellMeta};
use crate::effects::{EffectManager, EffectParamPreprocessor};
use crate::error::KernelError;
use crate::event::{IngressStamp, KernelEvent, ReducerEvent};
use crate::governance::{GovernanceManager, ManifestPatch, ProposalState};
use crate::governance_effects::GovernanceParamPreprocessor;
use crate::journal::fs::FsJournal;
use crate::journal::mem::MemJournal;
use crate::journal::{
    AppliedRecord, ApprovalDecisionRecord, ApprovedRecord, DomainEventRecord, EffectIntentRecord,
    EffectReceiptRecord, GovernanceRecord, IntentOriginRecord, Journal, JournalEntry, JournalKind,
    JournalRecord, JournalSeq, ManifestRecord, OwnedJournalEntry, PlanEndStatus, PlanEndedRecord,
    PlanResultRecord, PlanStartedRecord, ProposedRecord, ShadowReportRecord, SnapshotRecord,
};
use crate::manifest::{LoadedManifest, ManifestLoader};
use crate::plan::{PlanCompletionValue, PlanInstance, PlanRegistry, ReducerSchema};
use crate::policy::{AllowAllPolicy, RulePolicy};
use crate::pure::PureRegistry;
use crate::query::{Consistency, ReadMeta, StateRead, StateReader};
use crate::receipts::{ReducerEffectContext, build_reducer_receipt_event};
use crate::reducer::ReducerRegistry;
use crate::scheduler::{Scheduler, Task};
use crate::schema_value::cbor_to_expr_value;
use crate::secret::{PlaceholderSecretResolver, SharedSecretResolver};
use crate::shadow::{
    DeltaKind, LedgerDelta, LedgerKind, ShadowConfig, ShadowExecutor, ShadowHarness, ShadowSummary,
};
use crate::snapshot::{
    EffectIntentSnapshot, KernelSnapshot, PendingPlanReceiptSnapshot, PlanCompletionSnapshot,
    PlanResultSnapshot, ReducerReceiptSnapshot, ReducerStateEntry, SnapshotRootCompleteness,
    receipts_to_vecdeque,
};
use std::sync::Mutex;

mod bootstrap;
mod event_flow;
pub(crate) mod governance_runtime;
mod manifest_runtime;
mod plan_runtime;
mod query_api;
mod snapshot_replay;
#[cfg(test)]
pub(crate) mod test_support;

pub use crate::governance_utils::canonicalize_patch;

const RECENT_RECEIPT_CACHE: usize = 512;
const RECENT_PLAN_RESULT_CACHE: usize = 256;
const RECENT_PLAN_COMPLETION_CACHE: usize = 1024;
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
    plan_defs: HashMap<Name, DefPlan>,
    cap_defs: HashMap<Name, DefCap>,
    effect_defs: HashMap<Name, DefEffect>,
    policy_defs: HashMap<Name, DefPolicy>,
    schema_defs: HashMap<Name, DefSchema>,
    reducers: ReducerRegistry<S>,
    pures: Arc<Mutex<PureRegistry<S>>>,
    router: HashMap<String, Vec<RouteBinding>>,
    plan_registry: PlanRegistry,
    schema_index: Arc<SchemaIndex>,
    reducer_schemas: Arc<HashMap<Name, ReducerSchema>>,
    plan_cap_handles: HashMap<Name, Arc<HashMap<String, CapGrantResolution>>>,
    module_cap_bindings: HashMap<Name, HashMap<String, CapGrantResolution>>,
    plan_instances: HashMap<u64, PlanInstance>,
    plan_triggers: HashMap<String, Vec<PlanTriggerBinding>>,
    waiting_events: HashMap<String, Vec<u64>>,
    plan_wait_watchers: HashMap<u64, Vec<PlanWaitWatcher>>,
    completed_plan_outcomes: HashMap<u64, PlanCompletionOutcome>,
    completed_plan_order: VecDeque<u64>,
    pending_receipts: HashMap<[u8; 32], PendingPlanReceiptInfo>,
    pending_reducer_receipts: HashMap<[u8; 32], ReducerEffectContext>,
    recent_receipts: VecDeque<[u8; 32]>,
    recent_receipt_index: HashSet<[u8; 32]>,
    plan_results: VecDeque<PlanResultEntry>,
    scheduler: Scheduler,
    effect_manager: EffectManager,
    clock: KernelClock,
    reducer_state: HashMap<Name, ReducerState>,
    reducer_index_roots: HashMap<Name, Hash>,
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
pub struct PlanResultEntry {
    pub plan_name: String,
    pub plan_id: u64,
    pub output_schema: String,
    pub value_cbor: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingPlanReceiptInfo {
    plan_id: u64,
    effect_kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PlanWaitWatcher {
    parent_plan_id: u64,
}

#[derive(Clone, Debug, PartialEq)]
struct PlanCompletionOutcome {
    await_value: PlanCompletionValue,
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
        "defplan" | "plan" => Some("defplan"),
        "defcap" | "cap" => Some("defcap"),
        "defeffect" | "effect" => Some("defeffect"),
        "defpolicy" | "policy" => Some("defpolicy"),
        _ => None,
    }
}

#[derive(Clone, Debug)]
struct PlanTriggerBinding {
    plan: String,
    correlate_by: Option<String>,
    when: Option<Expr>,
    input_expr: Option<ExprOrValue>,
}

#[derive(Clone, Debug)]
enum EventWrap {
    Identity,
    Variant { tag: String },
}

#[derive(Clone, Debug)]
struct RouteBinding {
    reducer: Name,
    key_field: Option<String>,
    route_event_schema: String,
    reducer_event_schema: String,
    wrap: EventWrap,
}

#[derive(Clone)]
struct ReducerState {
    cell_cache: CellCache,
}

impl Default for ReducerState {
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

impl PlanResultEntry {
    fn new(plan_name: String, plan_id: u64, output_schema: String, value_cbor: Vec<u8>) -> Self {
        Self {
            plan_name,
            plan_id,
            output_schema,
            value_cbor,
        }
    }

    fn to_record(&self) -> PlanResultRecord {
        PlanResultRecord {
            plan_name: self.plan_name.clone(),
            plan_id: self.plan_id,
            output_schema: self.output_schema.clone(),
            value_cbor: self.value_cbor.clone(),
        }
    }

    fn to_snapshot(&self) -> PlanResultSnapshot {
        PlanResultSnapshot {
            plan_name: self.plan_name.clone(),
            plan_id: self.plan_id,
            output_schema: self.output_schema.clone(),
            value_cbor: self.value_cbor.clone(),
        }
    }

    fn from_record(record: PlanResultRecord) -> Self {
        Self::new(
            record.plan_name,
            record.plan_id,
            record.output_schema,
            record.value_cbor,
        )
    }

    fn from_snapshot(snapshot: PlanResultSnapshot) -> Self {
        Self::new(
            snapshot.plan_name,
            snapshot.plan_id,
            snapshot.output_schema,
            snapshot.value_cbor,
        )
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
    fn ensure_cell_index_root(&mut self, reducer: &Name) -> Result<Hash, KernelError> {
        if let Some(root) = self.reducer_index_roots.get(reducer) {
            return Ok(*root);
        }
        let index = CellIndex::new(self.store.as_ref());
        let root = index
            .empty()
            .map_err(|err| KernelError::SnapshotUnavailable(err.to_string()))?;
        self.reducer_index_roots.insert(reducer.clone(), root);
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
            IntentOriginRecord::Reducer { name } => {
                if self
                    .pending_reducer_receipts
                    .contains_key(&record.intent_hash)
                {
                    return Ok(());
                }
                self.pending_reducer_receipts
                    .entry(record.intent_hash)
                    .or_insert_with(|| {
                        ReducerEffectContext::new(name, effect_kind, params_cbor, None)
                    });
            }
            IntentOriginRecord::Plan { name: _, plan_id } => {
                self.reconcile_plan_replay_identity(plan_id, record.intent_hash);
                self.pending_receipts.insert(
                    record.intent_hash,
                    PendingPlanReceiptInfo {
                        plan_id,
                        effect_kind,
                    },
                );
            }
        }
        Ok(())
    }

    fn reconcile_plan_replay_identity(&mut self, recorded_plan_id: u64, intent_hash: [u8; 32]) {
        let matching_instance_id = self
            .plan_instances
            .iter()
            .find(|(_, instance)| instance.waiting_on_receipt(intent_hash))
            .map(|(id, _)| *id);

        if let Some(current_id) = matching_instance_id {
            if current_id != recorded_plan_id {
                if let Some(mut instance) = self.plan_instances.remove(&current_id) {
                    instance.id = recorded_plan_id;
                    instance.override_pending_receipt_hash(intent_hash);
                    self.plan_instances.insert(recorded_plan_id, instance);
                }
            } else if let Some(instance) = self.plan_instances.get_mut(&current_id) {
                instance.override_pending_receipt_hash(intent_hash);
            }
        }
    }

    pub fn reducer_state(&self, reducer: &str) -> Option<Vec<u8>> {
        self.reducer_state_bytes(reducer, None).ok().flatten()
    }

    /// Fetch reducer state bytes via the cell index (non-keyed reducers use the sentinel key).
    pub fn reducer_state_bytes(
        &self,
        reducer: &str,
        key: Option<&[u8]>,
    ) -> Result<Option<Vec<u8>>, KernelError> {
        let key = key.unwrap_or(MONO_KEY);
        let Some(root) = self.reducer_index_roots.get(reducer) else {
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

    /// Returns typed hash references reachable from reducer state by traversing the reducer's
    /// declared state schema. Hash-like text/bytes in opaque fields are ignored.
    pub fn reducer_state_typed_hash_refs(
        &self,
        reducer: &str,
        key: Option<&[u8]>,
    ) -> Result<Vec<Hash>, KernelError> {
        let Some(state_bytes) = self.reducer_state_bytes(reducer, key)? else {
            return Ok(Vec::new());
        };
        let module = self
            .module_defs
            .get(reducer)
            .ok_or_else(|| KernelError::ReducerNotFound(reducer.to_string()))?;
        let reducer_abi =
            module.abi.reducer.as_ref().ok_or_else(|| {
                KernelError::Manifest(format!("module '{reducer}' is not a reducer"))
            })?;
        let schema = self
            .schema_index
            .get(reducer_abi.state.as_str())
            .ok_or_else(|| {
                KernelError::Manifest(format!(
                    "state schema '{}' not found for reducer '{reducer}'",
                    reducer_abi.state
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
        if let Some(def) = self.plan_defs.get(name) {
            return Some(AirNode::Defplan(def.clone()));
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

        for (name, def) in self.plan_defs.iter() {
            let steps = def.steps.len();
            push_if(
                &mut entries,
                "defplan",
                name.as_str(),
                || DefListing {
                    kind: "defplan".into(),
                    name: name.clone(),
                    hash: hash_def(AirNode::Defplan(def.clone())),
                    cap_type: None,
                    params_schema: None,
                    receipt_schema: None,
                    plan_steps: Some(steps),
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

    /// List all cells for a reducer using the persisted CellIndex.
    ///
    /// Returns an empty Vec if the reducer has no cells yet.
    pub fn list_cells(&self, reducer: &str) -> Result<Vec<CellMeta>, KernelError> {
        let Some(root) = self.reducer_index_roots.get(reducer) else {
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

    /// Expose reducer index root hash (if present) for keyed reducers; useful for diagnostics/tests.
    pub fn reducer_index_root(&self, reducer: &str) -> Option<Hash> {
        self.reducer_index_roots.get(reducer).copied()
    }

    pub fn pending_reducer_receipts_snapshot(&self) -> Vec<ReducerReceiptSnapshot> {
        self.pending_reducer_receipts
            .iter()
            .map(|(hash, ctx)| ReducerReceiptSnapshot::from_context(*hash, ctx))
            .collect()
    }

    pub fn pending_plan_receipts(&self) -> Vec<(u64, [u8; 32])> {
        self.pending_receipts
            .iter()
            .map(|(hash, entry)| (entry.plan_id, *hash))
            .collect()
    }

    pub fn has_plan_instance(&self, id: u64) -> bool {
        self.plan_instances.contains_key(&id)
    }

    pub fn debug_plan_waits(&self) -> Vec<(u64, Vec<[u8; 32]>)> {
        self.plan_instances
            .iter()
            .map(|(id, instance)| (*id, instance.pending_receipt_hashes()))
            .collect()
    }

    pub fn debug_plan_waiting_events(&self) -> Vec<(u64, String)> {
        self.plan_instances
            .iter()
            .filter_map(|(id, instance)| {
                instance
                    .waiting_event_schema()
                    .map(|schema| (*id, schema.to_string()))
            })
            .collect()
    }

    pub fn plan_name_for_instance(&self, id: u64) -> Option<&str> {
        self.plan_instances
            .get(&id)
            .map(|instance| instance.plan.name.as_str())
    }

    pub fn dump_journal(&self) -> Result<Vec<OwnedJournalEntry>, KernelError> {
        Ok(self.journal.load_from(0)?)
    }

    pub fn governance(&self) -> &GovernanceManager {
        &self.governance
    }

    pub fn recent_plan_results(&self) -> Vec<PlanResultEntry> {
        self.plan_results.iter().cloned().collect()
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

fn determine_correlation_value(
    binding: &PlanTriggerBinding,
    input: &ExprValue,
    event_key: Option<&Vec<u8>>,
) -> Option<(Vec<u8>, ExprValue)> {
    if let Some(field) = &binding.correlate_by {
        if let Some(value) = extract_correlation_value(input, field) {
            let bytes = encode_correlation_bytes(&value);
            return Some((bytes, value));
        }
    }
    event_key.map(|key| (key.clone(), ExprValue::Bytes(key.clone())))
}

fn extract_correlation_value(value: &ExprValue, path: &str) -> Option<ExprValue> {
    let mut current = value;
    for segment in path.split('.') {
        if segment.is_empty() {
            continue;
        }
        current = match current {
            ExprValue::Record(map) => map.get(segment)?,
            ExprValue::Map(map) => map.get(&ValueKey::Text(segment.to_string()))?,
            _ => return None,
        };
    }
    Some(current.clone())
}

fn encode_correlation_bytes(value: &ExprValue) -> Vec<u8> {
    match value {
        ExprValue::Text(text) => text.as_bytes().to_vec(),
        ExprValue::Nat(n) => n.to_be_bytes().to_vec(),
        ExprValue::Int(i) => i.to_be_bytes().to_vec(),
        other => serde_cbor::to_vec(other).unwrap_or_default(),
    }
}

fn format_intent_hash(hash: &[u8; 32]) -> String {
    DigestHash::from_bytes(hash)
        .map(|h| h.to_hex())
        .unwrap_or_else(|_| format!("{:?}", hash))
}
