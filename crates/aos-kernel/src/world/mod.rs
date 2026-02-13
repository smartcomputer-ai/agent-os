use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use aos_air_exec::{Value as ExprValue, ValueKey};
use aos_air_types::{
    AirNode, DefCap, DefEffect, DefModule, DefPlan, DefPolicy, DefSchema, HashRef, Manifest, Name,
    NamedRef, PlanStepKind, SecretDecl, SecretEntry, TypeExpr, TypePrimitive, builtins,
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
    PlanResultRecord, ProposedRecord, ShadowReportRecord, SnapshotRecord,
};
use crate::manifest::{LoadedManifest, ManifestLoader};
use crate::plan::{PlanInstance, PlanRegistry, ReducerSchema};
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
    EffectIntentSnapshot, KernelSnapshot, PendingPlanReceiptSnapshot, PlanResultSnapshot,
    ReducerReceiptSnapshot, ReducerStateEntry, SnapshotRootCompleteness, receipts_to_vecdeque,
};
use std::sync::Mutex;

mod bootstrap;
mod event_flow;
pub(crate) mod governance_runtime;
mod manifest_runtime;
mod plan_runtime;
mod query_api;
mod snapshot_replay;

pub use crate::governance_utils::canonicalize_patch;

const RECENT_RECEIPT_CACHE: usize = 512;
const RECENT_PLAN_RESULT_CACHE: usize = 256;
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
    fn ensure_single_effect(output: &ReducerOutput) -> Result<(), KernelError> {
        if output.effects.len() > 1 {
            return Err(KernelError::ReducerOutput(
                "reducers may emit at most one effect per step; raise a domain intent and use a plan for additional effects".into(),
            ));
        }
        Ok(())
    }

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

    pub fn submit_domain_event(&mut self, schema: impl Into<String>, value: Vec<u8>) {
        let event = DomainEvent::new(schema.into(), value);
        let _ = self.process_domain_event(event);
    }

    pub fn submit_domain_event_with_key(
        &mut self,
        schema: impl Into<String>,
        value: Vec<u8>,
        key: Vec<u8>,
    ) {
        let event = DomainEvent::with_key(schema.into(), value, key);
        let _ = self.process_domain_event(event);
    }

    /// Submit a domain event and surface routing/validation errors (tests/fixtures helper).
    pub fn submit_domain_event_result(
        &mut self,
        schema: impl Into<String>,
        value: Vec<u8>,
    ) -> Result<(), KernelError> {
        let event = DomainEvent::new(schema.into(), value);
        self.process_domain_event(event)
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

    pub fn tick(&mut self) -> Result<(), KernelError> {
        if let Some(task) = self.scheduler.pop() {
            match task {
                Task::Reducer(event) => self.handle_reducer_event(event)?,
                Task::Plan(id) => self.handle_plan_task(id)?,
            }
        }
        Ok(())
    }

    fn process_domain_event(&mut self, event: DomainEvent) -> Result<(), KernelError> {
        let journal_height = self.journal.next_seq();
        let stamp = self.sample_ingress(journal_height)?;
        self.process_domain_event_with_ingress(event, stamp)
    }

    fn process_domain_event_with_ingress(
        &mut self,
        event: DomainEvent,
        stamp: IngressStamp,
    ) -> Result<(), KernelError> {
        Self::validate_entropy(&stamp.entropy)?;
        let event = self.normalize_domain_event(event)?;
        let routed = self.route_event(&event, &stamp)?;
        let mut event_for_plans = event.clone();
        if event_for_plans.key.is_none() {
            if let Some(key_bytes) = routed.iter().find_map(|ev| ev.event.key.clone()) {
                event_for_plans.key = Some(key_bytes);
            }
        }
        self.record_domain_event(&event_for_plans, &stamp)?;
        self.deliver_event_to_waiting_plans(&event_for_plans, &stamp)?;
        self.start_plans_for_event(&event_for_plans, &stamp)?;
        for ev in routed {
            self.scheduler.push_reducer(ev);
        }
        Ok(())
    }

    fn normalize_domain_event(&self, event: DomainEvent) -> Result<DomainEvent, KernelError> {
        let normalized =
            normalize_cbor_by_name(&self.schema_index, event.schema.as_str(), &event.value)
                .map_err(|err| {
                    KernelError::Manifest(format!(
                        "event '{}' payload failed validation: {err}",
                        event.schema
                    ))
                })?;
        Ok(DomainEvent {
            schema: event.schema,
            value: normalized.bytes,
            key: event.key,
        })
    }

    fn route_event(
        &self,
        event: &DomainEvent,
        stamp: &IngressStamp,
    ) -> Result<Vec<ReducerEvent>, KernelError> {
        let mut routed = Vec::new();
        let Some(bindings) = self.router.get(&event.schema) else {
            return Ok(routed);
        };
        let normalized =
            normalize_cbor_by_name(&self.schema_index, event.schema.as_str(), &event.value)
                .map_err(|err| {
                    KernelError::Manifest(format!(
                        "failed to decode event '{}' payload for routing: {err}",
                        event.schema
                    ))
                })?;
        let event_value = normalized.value;
        for binding in bindings {
            let module_def = self
                .module_defs
                .get(&binding.reducer)
                .ok_or_else(|| KernelError::ReducerNotFound(binding.reducer.clone()))?;
            let reducer_schema = self.reducer_schemas.get(&binding.reducer).ok_or_else(|| {
                KernelError::Manifest(format!(
                    "schema for reducer '{}' not found while routing event",
                    binding.reducer
                ))
            })?;
            let keyed = module_def.key_schema.is_some();

            match (keyed, &binding.key_field) {
                (true, None) => {
                    if event.key.is_none() {
                        return Err(KernelError::Manifest(format!(
                            "route to keyed reducer '{}' is missing key_field",
                            binding.reducer
                        )));
                    }
                }
                (false, Some(_)) => {
                    return Err(KernelError::Manifest(format!(
                        "route to non-keyed reducer '{}' provided key_field",
                        binding.reducer
                    )));
                }
                _ => {}
            }

            let wrapped_value = match &binding.wrap {
                EventWrap::Identity => event_value.clone(),
                EventWrap::Variant { tag } => CborValue::Map(BTreeMap::from([
                    (CborValue::Text("$tag".into()), CborValue::Text(tag.clone())),
                    (CborValue::Text("$value".into()), event_value.clone()),
                ])),
            };
            let normalized_for_reducer = normalize_value_with_schema(
                wrapped_value,
                &reducer_schema.event_schema,
                &self.schema_index,
            )
            .map_err(|err| {
                KernelError::Manifest(format!(
                    "failed to encode event '{}' for reducer '{}': {err}",
                    event.schema, binding.reducer
                ))
            })?;

            let key_bytes = if keyed {
                if let Some(field) = &binding.key_field {
                    let key_schema_ref = module_def
                        .key_schema
                        .as_ref()
                        .expect("keyed reducers have key_schema");
                    let key_schema =
                        self.schema_index
                            .get(key_schema_ref.as_str())
                            .ok_or_else(|| {
                                KernelError::Manifest(format!(
                                    "key schema '{}' not found for reducer '{}'",
                                    key_schema_ref.as_str(),
                                    binding.reducer
                                ))
                            })?;
                    let value_for_key = if binding.route_event_schema == event.schema {
                        &event_value
                    } else {
                        &normalized_for_reducer.value
                    };
                    Some(self.extract_key_bytes(
                        field,
                        key_schema,
                        value_for_key,
                        binding.route_event_schema.as_str(),
                    )?)
                } else {
                    event.key.clone()
                }
            } else {
                None
            };

            if let (Some(existing), Some(extracted)) = (&event.key, &key_bytes) {
                if existing != extracted {
                    return Err(KernelError::Manifest(format!(
                        "event '{}' carried key that differs from extracted key for reducer '{}'",
                        event.schema, binding.reducer
                    )));
                }
            }

            let mut routed_event = DomainEvent::new(
                binding.reducer_event_schema.clone(),
                normalized_for_reducer.bytes,
            );
            routed_event.key = event.key.clone();
            if let Some(bytes) = key_bytes.clone() {
                routed_event.key = Some(bytes);
            }
            routed.push(ReducerEvent {
                reducer: binding.reducer.clone(),
                event: routed_event,
                stamp: stamp.clone(),
            });
        }
        Ok(routed)
    }

    fn extract_key_bytes(
        &self,
        field: &str,
        key_schema: &TypeExpr,
        event_value: &CborValue,
        event_schema: &str,
    ) -> Result<Vec<u8>, KernelError> {
        let raw_value = extract_cbor_path(event_value, field).ok_or_else(|| {
            KernelError::Manifest(format!(
                "event '{event_schema}' missing key field '{field}'"
            ))
        })?;
        let normalized =
            normalize_value_with_schema(raw_value.clone(), key_schema, &self.schema_index)
                .map_err(|err| {
                    KernelError::Manifest(format!(
                        "event '{event_schema}' key field '{field}' failed validation: {err}"
                    ))
                })?;
        Ok(normalized.bytes)
    }

    fn handle_reducer_event(&mut self, event: ReducerEvent) -> Result<(), KernelError> {
        let reducer_name = event.reducer.clone();
        let (keyed, wants_context) = {
            let module_def = self
                .module_defs
                .get(&reducer_name)
                .ok_or_else(|| KernelError::ReducerNotFound(reducer_name.clone()))?;
            self.reducers.ensure_loaded(&reducer_name, module_def)?;
            (
                module_def.key_schema.is_some(),
                module_def
                    .abi
                    .reducer
                    .as_ref()
                    .and_then(|abi| abi.context.as_ref())
                    .is_some(),
            )
        };
        let key = event.event.key.clone();
        if keyed && key.is_none() {
            return Err(KernelError::Manifest(format!(
                "reducer '{reducer_name}' is keyed but event '{}' lacked a key",
                event.event.schema
            )));
        }
        if !keyed && key.is_some() {
            return Err(KernelError::Manifest(format!(
                "reducer '{reducer_name}' is not keyed but received a keyed event"
            )));
        }

        let mut index_root = self.reducer_index_roots.get(&reducer_name).copied();
        if keyed {
            index_root = Some(self.ensure_cell_index_root(&reducer_name)?);
        }

        let state_entry = self
            .reducer_state
            .entry(reducer_name.clone())
            .or_insert_with(ReducerState::default);
        let key_bytes: &[u8] = key.as_deref().unwrap_or(MONO_KEY);
        let current_state = if let Some(entry) = state_entry.cell_cache.get(key_bytes) {
            Some(entry.state.clone())
        } else if let Some(root) = index_root {
            let key_hash = Hash::of_bytes(key_bytes);
            let index = CellIndex::new(self.store.as_ref());
            if let Some(meta) = index.get(root, key_hash.as_bytes())? {
                let state_hash = Hash::from_bytes(&meta.state_hash)
                    .unwrap_or_else(|_| Hash::of_bytes(&meta.state_hash));
                let state = self.store.get_blob(state_hash)?;
                state_entry.cell_cache.insert(
                    key_bytes.to_vec(),
                    CellEntry {
                        state: state.clone(),
                        state_hash,
                        last_active_ns: meta.last_active_ns,
                    },
                );
                Some(state)
            } else {
                None
            }
        } else {
            None
        };

        let ctx_bytes = if wants_context {
            let event_hash = Hash::of_cbor(&event.event)
                .map_err(|err| KernelError::Manifest(err.to_string()))?
                .to_hex();
            let context = aos_wasm_abi::ReducerContext {
                now_ns: event.stamp.now_ns,
                logical_now_ns: event.stamp.logical_now_ns,
                journal_height: event.stamp.journal_height,
                entropy: event.stamp.entropy.clone(),
                event_hash,
                manifest_hash: event.stamp.manifest_hash.clone(),
                reducer: reducer_name.clone(),
                key: key.clone(),
                cell_mode: keyed,
            };
            Some(
                to_canonical_cbor(&context)
                    .map_err(|err| KernelError::Manifest(err.to_string()))?,
            )
        } else {
            None
        };
        self.effect_manager
            .set_cap_context(crate::effects::CapContext {
                logical_now_ns: event.stamp.logical_now_ns,
                journal_height: event.stamp.journal_height,
                manifest_hash: event.stamp.manifest_hash.clone(),
            });
        let input = ReducerInput {
            version: ABI_VERSION,
            state: current_state,
            event: event.event.clone(),
            ctx: ctx_bytes,
        };
        let output = self.reducers.invoke(&reducer_name, &input)?;
        self.handle_reducer_output(reducer_name.clone(), key, keyed, output)?;
        Ok(())
    }

    fn handle_reducer_output(
        &mut self,
        reducer_name: String,
        key: Option<Vec<u8>>,
        keyed: bool,
        output: ReducerOutput,
    ) -> Result<(), KernelError> {
        Self::ensure_single_effect(&output)?;

        let index_root = self.ensure_cell_index_root(&reducer_name)?;
        let mut new_index_root: Option<Hash> = None;

        let entry = self
            .reducer_state
            .entry(reducer_name.clone())
            .or_insert_with(ReducerState::default);

        let key_bytes = if keyed {
            key.clone().expect("key required for keyed reducer")
        } else {
            MONO_KEY.to_vec()
        };
        let key_hash = Hash::of_bytes(&key_bytes);

        match output.state {
            Some(state) => {
                let state_hash = self.store.put_blob(&state)?;
                let last_active_ns = self.journal.next_seq() as u64;
                let meta = CellMeta {
                    key_hash: *key_hash.as_bytes(),
                    key_bytes: key_bytes.clone(),
                    state_hash: *state_hash.as_bytes(),
                    size: state.len() as u64,
                    last_active_ns,
                };
                let index = CellIndex::new(self.store.as_ref());
                let new_root = index.upsert(index_root, meta)?;
                new_index_root = Some(new_root);
                entry.cell_cache.insert(
                    key_bytes,
                    CellEntry {
                        state,
                        state_hash,
                        last_active_ns,
                    },
                );
            }
            None => {
                let index = CellIndex::new(self.store.as_ref());
                let (new_root, removed) = index.delete(index_root, key_hash.as_bytes())?;
                if removed {
                    new_index_root = Some(new_root);
                    entry.cell_cache.remove(&key_bytes);
                }
            }
        }
        if let Some(root) = new_index_root {
            self.reducer_index_roots.insert(reducer_name.clone(), root);
        }
        for event in output.domain_events {
            self.process_domain_event(event)?;
        }
        for effect in &output.effects {
            let slot = effect.cap_slot.clone().unwrap_or_else(|| "default".into());
            let bound_grant = self
                .module_cap_bindings
                .get(&reducer_name)
                .and_then(|binding| binding.get(&slot));
            let default_grant = if bound_grant.is_none() && slot == "default" {
                self.effect_manager
                    .unique_grant_for_effect_kind(effect.kind.as_str())?
            } else {
                None
            };
            let grant = bound_grant
                .or_else(|| default_grant.as_ref())
                .ok_or_else(|| KernelError::CapabilityBindingMissing {
                    reducer: reducer_name.clone(),
                    slot: slot.clone(),
                })?;
            let intent = match self.effect_manager.enqueue_reducer_effect_with_grant(
                &reducer_name,
                grant,
                effect,
            ) {
                Ok(intent) => intent,
                Err(err) => {
                    self.record_decisions()?;
                    return Err(err);
                }
            };
            self.record_decisions()?;
            self.record_effect_intent(
                &intent,
                IntentOriginRecord::Reducer {
                    name: reducer_name.clone(),
                },
            )?;
            self.pending_reducer_receipts.insert(
                intent.intent_hash,
                ReducerEffectContext::new(
                    reducer_name.clone(),
                    effect.kind.clone(),
                    effect.params_cbor.clone(),
                    key.clone(),
                ),
            );
        }
        Ok(())
    }

    pub fn drain_effects(&mut self) -> Vec<aos_effects::EffectIntent> {
        self.effect_manager.drain()
    }

    /// Returns true when the effect queue is non-empty and a cycle is needed.
    pub fn has_pending_effects(&self) -> bool {
        self.effect_manager.has_pending()
    }

    pub fn restore_effect_queue(&mut self, intents: Vec<aos_effects::EffectIntent>) {
        self.effect_manager.restore_queue(intents);
    }

    pub fn create_snapshot(&mut self) -> Result<(), KernelError> {
        self.tick_until_idle()?;
        if !self.scheduler.is_empty() {
            return Err(KernelError::SnapshotUnavailable(
                "scheduler must be idle before snapshot".into(),
            ));
        }
        let height = self.journal.next_seq();
        let reducer_state: Vec<ReducerStateEntry> = Vec::new();
        let recent_receipts: Vec<[u8; 32]> = self.recent_receipts.iter().cloned().collect();
        let plan_instances = self
            .plan_instances
            .values()
            .map(|instance| instance.snapshot())
            .collect();
        let pending_plan_receipts = self
            .pending_receipts
            .iter()
            .map(|(hash, entry)| PendingPlanReceiptSnapshot {
                plan_id: entry.plan_id,
                intent_hash: *hash,
                effect_kind: entry.effect_kind.clone(),
            })
            .collect();
        let waiting_events = self
            .waiting_events
            .iter()
            .map(|(schema, ids)| (schema.clone(), ids.clone()))
            .collect();
        let queued_effects = self
            .effect_manager
            .queued()
            .iter()
            .map(EffectIntentSnapshot::from_intent)
            .collect();
        let reducer_index_roots = self
            .reducer_index_roots
            .iter()
            .map(|(name, hash)| (name.clone(), *hash.as_bytes()))
            .collect();
        let pending_reducer_receipts = self
            .pending_reducer_receipts
            .iter()
            .map(|(hash, ctx)| ReducerReceiptSnapshot::from_context(*hash, ctx))
            .collect();
        let plan_results: Vec<PlanResultSnapshot> = self
            .plan_results
            .iter()
            .map(|entry| entry.to_snapshot())
            .collect();
        let logical_now_ns = self.effect_manager.logical_now_ns();
        let mut snapshot = KernelSnapshot::new(
            height,
            reducer_state,
            recent_receipts,
            plan_instances,
            pending_plan_receipts,
            waiting_events,
            self.scheduler.next_plan_id(),
            queued_effects,
            pending_reducer_receipts,
            plan_results,
            logical_now_ns,
            Some(*self.manifest_hash.as_bytes()),
        );
        snapshot.set_reducer_index_roots(reducer_index_roots);
        let root_completeness = SnapshotRootCompleteness {
            manifest_hash: Some(self.manifest_hash.as_bytes().to_vec()),
            reducer_state_roots: snapshot
                .reducer_state_entries()
                .iter()
                .map(|entry| entry.state_hash)
                .collect(),
            cell_index_roots: snapshot
                .reducer_index_roots()
                .iter()
                .map(|(_, root)| *root)
                .collect(),
            workspace_roots: self
                .workspace_roots
                .iter()
                .map(|hash| *hash.as_bytes())
                .collect(),
            pinned_roots: self
                .pinned_roots
                .iter()
                .map(|hash| *hash.as_bytes())
                .collect(),
        };
        snapshot.set_root_completeness(root_completeness);
        self.validate_snapshot_root_completeness(&snapshot)?;
        let bytes = serde_cbor::to_vec(&snapshot)
            .map_err(|err| KernelError::SnapshotDecode(err.to_string()))?;
        let hash = self.store.put_blob(&bytes)?;
        let baseline = SnapshotRecord {
            snapshot_ref: hash.to_hex(),
            height,
            logical_time_ns: logical_now_ns,
            receipt_horizon_height: self.receipt_horizon_height_for_baseline(height),
            manifest_hash: Some(self.manifest_hash.to_hex()),
        };
        self.append_record(JournalRecord::Snapshot(baseline.clone()))?;
        self.snapshot_index
            .insert(height, (hash, Some(self.manifest_hash)));
        if self.validate_baseline_promotion(&baseline).is_ok() {
            self.active_baseline = Some(baseline);
        }
        self.last_snapshot_hash = Some(hash);
        self.last_snapshot_height = Some(height);
        Ok(())
    }

    fn replay_existing_entries(&mut self) -> Result<(), KernelError> {
        let entries = self.journal.load_from(0)?;
        if entries.is_empty() {
            return Ok(());
        }
        let mut resume_seq: Option<JournalSeq> = None;
        let mut latest_promotable_baseline: Option<SnapshotRecord> = None;
        for entry in &entries {
            if matches!(entry.kind, JournalKind::Snapshot) {
                let record: JournalRecord = serde_cbor::from_slice(&entry.payload)
                    .map_err(|err| KernelError::Journal(err.to_string()))?;
                if let JournalRecord::Snapshot(snapshot) = record {
                    if let Ok(hash) = Hash::from_hex_str(&snapshot.snapshot_ref) {
                        let manifest_hash = snapshot
                            .manifest_hash
                            .as_ref()
                            .and_then(|s| Hash::from_hex_str(s).ok());
                        self.snapshot_index
                            .insert(snapshot.height, (hash, manifest_hash));
                    }
                    if self.validate_baseline_promotion(&snapshot).is_ok() {
                        latest_promotable_baseline = Some(snapshot.clone());
                    }
                }
            }
        }
        if let Some(snapshot) = latest_promotable_baseline {
            resume_seq = Some(snapshot.height);
            self.active_baseline = Some(snapshot.clone());
            self.last_snapshot_height = Some(snapshot.height);
            self.load_snapshot(&snapshot)?;
        }
        self.suppress_journal = true;
        for entry in entries {
            if resume_seq.map_or(false, |seq| entry.seq < seq) {
                continue;
            }
            let record: JournalRecord = serde_cbor::from_slice(&entry.payload)
                .map_err(|err| KernelError::Journal(err.to_string()))?;
            self.apply_replay_record(record)?;
        }
        self.tick_until_idle()?;
        self.suppress_journal = false;
        Ok(())
    }

    fn apply_replay_record(&mut self, record: JournalRecord) -> Result<(), KernelError> {
        match record {
            JournalRecord::DomainEvent(event) => {
                self.sync_logical_from_record(event.logical_now_ns);
                let stamp = IngressStamp {
                    now_ns: event.now_ns,
                    logical_now_ns: event.logical_now_ns,
                    entropy: event.entropy,
                    journal_height: event.journal_height,
                    manifest_hash: if event.manifest_hash.is_empty() {
                        self.manifest_hash.to_hex()
                    } else {
                        event.manifest_hash
                    },
                };
                let event = DomainEvent {
                    schema: event.schema,
                    value: event.value,
                    key: event.key,
                };
                self.process_domain_event_with_ingress(event, stamp)?;
                self.tick_until_idle()?;
            }
            JournalRecord::EffectIntent(record) => {
                self.restore_effect_intent(record)?;
            }
            JournalRecord::EffectReceipt(record) => {
                self.sync_logical_from_record(record.logical_now_ns);
                let stamp = IngressStamp {
                    now_ns: record.now_ns,
                    logical_now_ns: record.logical_now_ns,
                    entropy: record.entropy,
                    journal_height: record.journal_height,
                    manifest_hash: if record.manifest_hash.is_empty() {
                        self.manifest_hash.to_hex()
                    } else {
                        record.manifest_hash
                    },
                };
                let receipt = EffectReceipt {
                    intent_hash: record.intent_hash,
                    adapter_id: record.adapter_id,
                    status: record.status,
                    payload_cbor: record.payload_cbor,
                    cost_cents: record.cost_cents,
                    signature: record.signature,
                };
                self.handle_receipt_with_ingress(receipt, stamp)?;
                self.tick_until_idle()?;
            }
            JournalRecord::CapDecision(_) => {
                // Cap decisions are audit-only; runtime state is rebuilt via replay.
            }
            JournalRecord::PolicyDecision(_) => {
                // Policy decisions are audit-only; runtime state is rebuilt via replay.
            }
            JournalRecord::Manifest(record) => {
                let hash = Hash::from_hex_str(&record.manifest_hash).map_err(|err| {
                    KernelError::Manifest(format!("invalid manifest hash: {err}"))
                })?;
                if hash != self.manifest_hash {
                    let loaded = ManifestLoader::load_from_hash(self.store.as_ref(), hash)?;
                    self.apply_loaded_manifest(loaded, false)?;
                }
            }
            JournalRecord::Snapshot(_) => {
                // already handled separately
            }
            JournalRecord::Governance(record) => {
                self.governance.apply_record(&record);
            }
            JournalRecord::PlanResult(record) => {
                self.restore_plan_result(record);
            }
            JournalRecord::PlanEnded(_) => {
                // No runtime side effects to restore; plan instances are not replayed from journal.
            }
            _ => {}
        }
        Ok(())
    }

    fn load_snapshot(&mut self, record: &SnapshotRecord) -> Result<(), KernelError> {
        self.validate_baseline_promotion(record)?;
        if record.manifest_hash.is_none() {
            return Err(KernelError::SnapshotUnavailable(
                "snapshot record missing manifest_hash".into(),
            ));
        }
        let hash = Hash::from_hex_str(&record.snapshot_ref)
            .map_err(|err| KernelError::SnapshotDecode(err.to_string()))?;
        let bytes = self.store.get_blob(hash)?;
        let snapshot: KernelSnapshot = serde_cbor::from_slice(&bytes)
            .map_err(|err| KernelError::SnapshotDecode(err.to_string()))?;
        self.validate_snapshot_root_completeness(&snapshot)?;
        self.last_snapshot_height = Some(record.height);
        self.last_snapshot_hash = Some(hash);
        self.active_baseline = Some(record.clone());
        if let Some(manifest_hex) = record.manifest_hash.as_ref() {
            if let Ok(h) = Hash::from_hex_str(manifest_hex) {
                if h != self.manifest_hash {
                    let loaded = ManifestLoader::load_from_hash(self.store.as_ref(), h)?;
                    self.apply_loaded_manifest(loaded, false)?;
                }
            }
        }
        self.snapshot_index.insert(
            record.height,
            (
                hash,
                record
                    .manifest_hash
                    .as_ref()
                    .and_then(|s| Hash::from_hex_str(s).ok()),
            ),
        );
        self.apply_snapshot(snapshot)
    }

    fn ensure_active_baseline(&mut self) -> Result<(), KernelError> {
        if self.active_baseline.is_some() {
            return Ok(());
        }
        self.create_snapshot()?;
        if self.active_baseline.is_none() {
            return Err(KernelError::SnapshotUnavailable(
                "failed to establish active baseline due to receipt-horizon precondition".into(),
            ));
        }
        Ok(())
    }

    fn receipt_horizon_height_for_baseline(&self, height: JournalSeq) -> Option<JournalSeq> {
        if self.pending_receipts.is_empty() && self.pending_reducer_receipts.is_empty() {
            Some(height)
        } else {
            None
        }
    }

    fn validate_baseline_promotion(&self, record: &SnapshotRecord) -> Result<(), KernelError> {
        let Some(horizon) = record.receipt_horizon_height else {
            return Err(KernelError::SnapshotUnavailable(
                "baseline promotion requires receipt_horizon_height".into(),
            ));
        };
        if horizon != record.height {
            return Err(KernelError::SnapshotUnavailable(format!(
                "baseline receipt_horizon_height ({horizon}) must equal baseline height ({})",
                record.height
            )));
        }
        Ok(())
    }

    fn validate_snapshot_root_completeness(
        &self,
        snapshot: &KernelSnapshot,
    ) -> Result<(), KernelError> {
        let roots = snapshot.root_completeness();
        let Some(snapshot_manifest_hash) = snapshot.manifest_hash() else {
            return Err(KernelError::SnapshotUnavailable(
                "snapshot root completeness missing manifest_hash".into(),
            ));
        };
        let Some(roots_manifest_hash) = roots.manifest_hash.as_ref() else {
            return Err(KernelError::SnapshotUnavailable(
                "root completeness missing manifest_hash".into(),
            ));
        };
        if roots_manifest_hash.as_slice() != snapshot_manifest_hash {
            return Err(KernelError::SnapshotUnavailable(
                "root completeness manifest_hash mismatch".into(),
            ));
        }

        let state_roots: HashSet<[u8; 32]> = roots.reducer_state_roots.iter().cloned().collect();
        for entry in snapshot.reducer_state_entries() {
            if !state_roots.contains(&entry.state_hash) {
                return Err(KernelError::SnapshotUnavailable(
                    "root completeness missing reducer state root".into(),
                ));
            }
        }

        let index_roots: HashSet<[u8; 32]> = roots.cell_index_roots.iter().cloned().collect();
        for (_, root) in snapshot.reducer_index_roots() {
            if !index_roots.contains(root) {
                return Err(KernelError::SnapshotUnavailable(
                    "root completeness missing reducer cell_index_root".into(),
                ));
            }
        }
        Ok(())
    }

    fn apply_snapshot(&mut self, snapshot: KernelSnapshot) -> Result<(), KernelError> {
        self.reducer_index_roots = snapshot
            .reducer_index_roots()
            .iter()
            .filter_map(|(name, bytes)| Hash::from_bytes(bytes).ok().map(|h| (name.clone(), h)))
            .collect();

        if let Some(bytes) = snapshot.manifest_hash() {
            if let Ok(hash) = Hash::from_bytes(bytes) {
                self.manifest_hash = hash;
            }
        }

        let mut restored: HashMap<Name, ReducerState> = HashMap::new();
        for entry in snapshot.reducer_state_entries().iter().cloned() {
            // Ensure blobs are present in store for deterministic reloads.
            self.store.put_blob(&entry.state)?;
            let state_entry = restored
                .entry(entry.reducer.clone())
                .or_insert_with(ReducerState::default);
            let state_hash = Hash::from_bytes(&entry.state_hash)
                .unwrap_or_else(|_| Hash::of_bytes(&entry.state));
            let key_bytes = entry.key.unwrap_or_else(|| MONO_KEY.to_vec());
            let key_hash = Hash::of_bytes(&key_bytes);
            let root = self.ensure_cell_index_root(&entry.reducer)?;
            let meta = CellMeta {
                key_hash: *key_hash.as_bytes(),
                key_bytes: key_bytes.clone(),
                state_hash: *state_hash.as_bytes(),
                size: entry.state.len() as u64,
                last_active_ns: entry.last_active_ns,
            };
            let index = CellIndex::new(self.store.as_ref());
            let new_root = index.upsert(root, meta)?;
            self.reducer_index_roots
                .insert(entry.reducer.clone(), new_root);
            state_entry.cell_cache.insert(
                key_bytes,
                CellEntry {
                    state: entry.state.clone(),
                    state_hash,
                    last_active_ns: entry.last_active_ns,
                },
            );
        }
        self.reducer_state = restored;
        let (deque, set) = receipts_to_vecdeque(snapshot.recent_receipts(), RECENT_RECEIPT_CACHE);
        self.recent_receipts = deque;
        self.recent_receipt_index = set;

        self.plan_instances.clear();
        for inst_snapshot in snapshot.plan_instances().iter().cloned() {
            let plan = self
                .plan_registry
                .get(&inst_snapshot.name)
                .ok_or_else(|| {
                    KernelError::SnapshotUnavailable(format!(
                        "plan '{}' missing while applying snapshot",
                        inst_snapshot.name
                    ))
                })?
                .clone();
            let cap_handles = self
                .plan_cap_handles
                .get(&inst_snapshot.name)
                .ok_or_else(|| {
                    KernelError::SnapshotUnavailable(format!(
                        "plan '{}' missing cap bindings while applying snapshot",
                        inst_snapshot.name
                    ))
                })?
                .clone();
            let instance = PlanInstance::from_snapshot(
                inst_snapshot,
                plan,
                self.schema_index.clone(),
                cap_handles,
            );
            self.plan_instances.insert(instance.id, instance);
        }

        self.pending_receipts = snapshot
            .pending_plan_receipts()
            .iter()
            .cloned()
            .map(|snap| {
                (
                    snap.intent_hash,
                    PendingPlanReceiptInfo {
                        plan_id: snap.plan_id,
                        effect_kind: snap.effect_kind,
                    },
                )
            })
            .collect();
        self.pending_reducer_receipts = snapshot
            .pending_reducer_receipts()
            .iter()
            .cloned()
            .map(|snap| (snap.intent_hash, snap.into_context()))
            .collect();
        self.waiting_events = snapshot.waiting_events().iter().cloned().collect();

        self.effect_manager.restore_queue(
            snapshot
                .queued_effects()
                .iter()
                .cloned()
                .map(|snap| snap.into_intent())
                .collect(),
        );
        self.effect_manager
            .update_logical_now_ns(snapshot.logical_now_ns());
        self.clock
            .sync_logical_min(self.effect_manager.logical_now_ns());

        self.plan_results.clear();
        for result_snapshot in snapshot.plan_results().iter().cloned() {
            self.push_plan_result_entry(PlanResultEntry::from_snapshot(result_snapshot));
        }

        self.scheduler.clear();
        self.scheduler.set_next_plan_id(snapshot.next_plan_id());

        Ok(())
    }

    fn restore_plan_result(&mut self, record: PlanResultRecord) {
        self.push_plan_result_entry(PlanResultEntry::from_record(record));
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

    pub fn tick_until_idle(&mut self) -> Result<(), KernelError> {
        while !self.scheduler.is_empty() {
            self.tick()?;
        }
        Ok(())
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

    /// Return snapshot record (hash + manifest hash) for an exact height, if known.
    fn snapshot_at_height(&self, height: JournalSeq) -> Option<(Hash, Option<Hash>)> {
        self.snapshot_index.get(&height).cloned()
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

    pub fn tail_scan_after(&self, height: JournalSeq) -> Result<TailScan, KernelError> {
        let head = self.journal.next_seq();
        let from_seq = if self.last_snapshot_height.is_none() && height == 0 {
            0
        } else {
            height.saturating_add(1)
        };
        if from_seq >= head {
            return Ok(TailScan {
                from: height,
                to: head,
                entries: Vec::new(),
                intents: Vec::new(),
                receipts: Vec::new(),
            });
        }

        let entries = self.journal.load_from(from_seq)?;
        let mut scan = TailScan {
            from: height,
            to: head,
            entries: Vec::new(),
            intents: Vec::new(),
            receipts: Vec::new(),
        };

        for entry in entries {
            let record = decode_tail_record(entry.kind, &entry.payload)?;
            scan.entries.push(TailEntry {
                seq: entry.seq,
                kind: entry.kind,
                record: record.clone(),
            });

            match record {
                JournalRecord::EffectIntent(record) => {
                    scan.intents.push(TailIntent {
                        seq: entry.seq,
                        record,
                    });
                }
                JournalRecord::EffectReceipt(record) => {
                    scan.receipts.push(TailReceipt {
                        seq: entry.seq,
                        record,
                    });
                }
                _ => {}
            }
        }

        Ok(scan)
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

    pub fn submit_proposal(
        &mut self,
        patch: ManifestPatch,
        description: Option<String>,
    ) -> Result<u64, KernelError> {
        let proposal_id = self.governance.alloc_proposal_id();

        let canonical_patch = canonicalize_patch(self.store.as_ref(), patch)?;

        for node in &canonical_patch.nodes {
            self.store.put_node(node)?;
        }
        self.store.put_node(&canonical_patch.manifest)?;

        let patch_bytes = to_canonical_cbor(&canonical_patch)
            .map_err(|err| KernelError::Manifest(format!("encode patch: {err}")))?;
        let patch_hash = self.store.put_blob(&patch_bytes)?;
        let record = GovernanceRecord::Proposed(ProposedRecord {
            proposal_id,
            description,
            patch_hash: patch_hash.to_hex(),
        });
        self.append_record(JournalRecord::Governance(record.clone()))?;
        self.governance.apply_record(&record);
        Ok(proposal_id)
    }

    pub fn run_shadow(
        &mut self,
        proposal_id: u64,
        harness: Option<ShadowHarness>,
    ) -> Result<ShadowSummary, KernelError> {
        let proposal = self
            .governance
            .proposals()
            .get(&proposal_id)
            .ok_or(KernelError::ProposalNotFound(proposal_id))?
            .clone();
        match proposal.state {
            ProposalState::Applied => return Err(KernelError::ProposalAlreadyApplied(proposal_id)),
            ProposalState::Submitted | ProposalState::Shadowed | ProposalState::Approved => {}
            ProposalState::Rejected => {
                return Err(KernelError::ProposalStateInvalid {
                    proposal_id,
                    state: proposal.state,
                    required: "not rejected",
                });
            }
        }
        let patch = self.load_manifest_patch(&proposal.patch_hash)?;
        let config = ShadowConfig {
            proposal_id,
            patch,
            patch_hash: proposal.patch_hash.clone(),
            harness,
        };
        let mut summary = ShadowExecutor::run(self.store.clone(), &config)?;
        summary.ledger_deltas = Self::compute_ledger_deltas(&self.manifest, &config.patch.manifest);
        let record = GovernanceRecord::ShadowReport(ShadowReportRecord {
            proposal_id,
            patch_hash: proposal.patch_hash.clone(),
            manifest_hash: summary.manifest_hash.clone(),
            effects_predicted: summary.predicted_effects.clone(),
            pending_receipts: summary.pending_receipts.clone(),
            plan_results: summary.plan_results.clone(),
            ledger_deltas: summary.ledger_deltas.clone(),
        });
        self.append_record(JournalRecord::Governance(record.clone()))?;
        self.governance.apply_record(&record);
        Ok(summary)
    }

    pub fn approve_proposal(
        &mut self,
        proposal_id: u64,
        approver: impl Into<String>,
    ) -> Result<(), KernelError> {
        self.decide_proposal(proposal_id, approver, ApprovalDecisionRecord::Approve)
    }

    pub fn reject_proposal(
        &mut self,
        proposal_id: u64,
        approver: impl Into<String>,
    ) -> Result<(), KernelError> {
        self.decide_proposal(proposal_id, approver, ApprovalDecisionRecord::Reject)
    }

    fn decide_proposal(
        &mut self,
        proposal_id: u64,
        approver: impl Into<String>,
        decision: ApprovalDecisionRecord,
    ) -> Result<(), KernelError> {
        let proposal = self
            .governance
            .proposals()
            .get(&proposal_id)
            .ok_or(KernelError::ProposalNotFound(proposal_id))?
            .clone();
        if matches!(proposal.state, ProposalState::Applied) {
            return Err(KernelError::ProposalAlreadyApplied(proposal_id));
        }
        if !matches!(
            proposal.state,
            ProposalState::Shadowed | ProposalState::Approved
        ) {
            return Err(KernelError::ProposalStateInvalid {
                proposal_id,
                state: proposal.state,
                required: "shadowed",
            });
        }
        let record = GovernanceRecord::Approved(ApprovedRecord {
            proposal_id,
            patch_hash: proposal.patch_hash.clone(),
            approver: approver.into(),
            decision,
        });
        self.append_record(JournalRecord::Governance(record.clone()))?;
        self.governance.apply_record(&record);
        Ok(())
    }

    pub fn apply_proposal(&mut self, proposal_id: u64) -> Result<(), KernelError> {
        let proposal = self
            .governance
            .proposals()
            .get(&proposal_id)
            .ok_or(KernelError::ProposalNotFound(proposal_id))?
            .clone();
        if matches!(proposal.state, ProposalState::Applied) {
            return Err(KernelError::ProposalAlreadyApplied(proposal_id));
        }
        if !matches!(proposal.state, ProposalState::Approved) {
            return Err(KernelError::ProposalStateInvalid {
                proposal_id,
                state: proposal.state,
                required: "approved",
            });
        }
        let patch = self.load_manifest_patch(&proposal.patch_hash)?;
        self.swap_manifest(&patch)?;

        let manifest_bytes = to_canonical_cbor(&patch.manifest)
            .map_err(|err| KernelError::Manifest(format!("encode manifest: {err}")))?;
        let manifest_hash_new = Hash::of_bytes(&manifest_bytes).to_hex();

        let record = GovernanceRecord::Applied(AppliedRecord {
            proposal_id,
            patch_hash: proposal.patch_hash.clone(),
            manifest_hash_new,
        });
        self.append_record(JournalRecord::Governance(record.clone()))?;
        self.governance.apply_record(&record);
        Ok(())
    }

    /// Apply a manifest patch directly without governance (dev mode).
    ///
    /// The patch is canonicalized, stored in the CAS, and swapped into the live kernel.
    pub fn apply_patch_direct(&mut self, patch: ManifestPatch) -> Result<String, KernelError> {
        let canonical = canonicalize_patch(self.store.as_ref(), patch)?;

        for node in &canonical.nodes {
            self.store.put_node(node)?;
        }
        self.store.put_node(&canonical.manifest)?;
        self.store
            .put_node(&AirNode::Manifest(canonical.manifest.clone()))?;

        self.swap_manifest(&canonical)?;
        Ok(self.manifest_hash.to_hex())
    }

    fn load_manifest_patch(&self, hash_hex: &str) -> Result<ManifestPatch, KernelError> {
        let hash = Hash::from_hex_str(hash_hex)
            .map_err(|err| KernelError::Manifest(format!("invalid patch hash: {err}")))?;
        let bytes = self.store.get_blob(hash)?;
        let patch: ManifestPatch = serde_cbor::from_slice(&bytes)
            .map_err(|err| KernelError::Manifest(format!("decode patch: {err}")))?;
        Ok(patch)
    }

    fn swap_manifest(&mut self, patch: &ManifestPatch) -> Result<(), KernelError> {
        let loaded = patch.to_loaded_manifest(self.store.as_ref())?;
        self.apply_loaded_manifest(loaded, true)
    }

    fn apply_loaded_manifest(
        &mut self,
        loaded: LoadedManifest,
        record_manifest: bool,
    ) -> Result<(), KernelError> {
        let runtime = manifest_runtime::assemble_runtime(self.store.as_ref(), &loaded)?;

        self.manifest = loaded.manifest;
        let manifest_bytes = to_canonical_cbor(&self.manifest)
            .map_err(|err| KernelError::Manifest(format!("encode manifest: {err}")))?;
        self.manifest_hash = Hash::of_bytes(&manifest_bytes);
        self.secrets = loaded.secrets;
        self.module_defs = loaded.modules;
        self.plan_defs = loaded.plans;
        self.cap_defs = loaded.caps;
        self.effect_defs = loaded.effects;
        self.policy_defs = loaded.policies;
        self.schema_defs = loaded.schemas;
        self.plan_registry = runtime.plan_registry;
        self.router = runtime.router;
        self.plan_triggers = runtime.plan_triggers;

        self.plan_instances.clear();
        self.waiting_events.clear();
        self.pending_receipts.clear();
        self.pending_reducer_receipts.clear();
        self.recent_receipts.clear();
        self.recent_receipt_index.clear();
        self.plan_results.clear();

        self.schema_index = runtime.schema_index.clone();
        self.reducer_schemas = runtime.reducer_schemas;
        self.secret_resolver = ensure_secret_resolver(
            !self.secrets.is_empty(),
            self.secret_resolver.clone(),
            self.allow_placeholder_secrets,
        )?;
        let secret_catalog = if self.secrets.is_empty() {
            None
        } else {
            Some(crate::secret::SecretCatalog::new(&self.secrets))
        };
        let enforcer_invoker: Option<Arc<dyn CapEnforcerInvoker>> = Some(Arc::new(
            PureCapEnforcer::new(Arc::new(self.module_defs.clone()), self.pures.clone()),
        ));
        let param_preprocessor: Option<Arc<dyn EffectParamPreprocessor>> = Some(Arc::new(
            GovernanceParamPreprocessor::new(self.store.clone(), self.manifest.clone()),
        ));
        self.effect_manager = EffectManager::new(
            runtime.capability_resolver,
            runtime.policy_gate,
            runtime.effect_catalog,
            runtime.schema_index.clone(),
            param_preprocessor,
            enforcer_invoker,
            secret_catalog,
            self.secret_resolver.clone(),
        );
        self.plan_cap_handles = runtime.plan_cap_handles;
        self.module_cap_bindings = runtime.module_cap_bindings;
        if record_manifest {
            self.record_manifest()?;
        }
        Ok(())
    }

    fn compute_ledger_deltas(current: &Manifest, candidate: &Manifest) -> Vec<LedgerDelta> {
        let mut deltas = Vec::new();
        deltas.extend(
            crate::governance_utils::diff_named_refs(&current.caps, &candidate.caps)
                .into_iter()
                .map(|delta| LedgerDelta {
                    ledger: LedgerKind::Capability,
                    name: delta.name,
                    change: match delta.change {
                        crate::governance_utils::NamedRefDiffKind::Added => DeltaKind::Added,
                        crate::governance_utils::NamedRefDiffKind::Removed => DeltaKind::Removed,
                        crate::governance_utils::NamedRefDiffKind::Changed => DeltaKind::Changed,
                    },
                }),
        );
        deltas.extend(
            crate::governance_utils::diff_named_refs(&current.policies, &candidate.policies)
                .into_iter()
                .map(|delta| LedgerDelta {
                    ledger: LedgerKind::Policy,
                    name: delta.name,
                    change: match delta.change {
                        crate::governance_utils::NamedRefDiffKind::Added => DeltaKind::Added,
                        crate::governance_utils::NamedRefDiffKind::Removed => DeltaKind::Removed,
                        crate::governance_utils::NamedRefDiffKind::Changed => DeltaKind::Changed,
                    },
                }),
        );

        deltas.sort_by(|a, b| {
            let ledger_a = match a.ledger {
                LedgerKind::Capability => 0,
                LedgerKind::Policy => 1,
            };
            let ledger_b = match b.ledger {
                LedgerKind::Capability => 0,
                LedgerKind::Policy => 1,
            };
            (ledger_a, &a.name, format!("{:?}", a.change)).cmp(&(
                ledger_b,
                &b.name,
                format!("{:?}", b.change),
            ))
        });
        deltas
    }

    fn secret_resolver(&self) -> Option<SharedSecretResolver> {
        self.secret_resolver.clone()
    }

    fn start_plans_for_event(
        &mut self,
        event: &DomainEvent,
        stamp: &IngressStamp,
    ) -> Result<(), KernelError> {
        if let Some(plan_bindings) = self.plan_triggers.get(&event.schema) {
            for binding in plan_bindings {
                if let Some(plan_def) = self.plan_registry.get(&binding.plan) {
                    let normalized = normalize_cbor_by_name(
                        &self.schema_index,
                        event.schema.as_str(),
                        &event.value,
                    )
                    .map_err(|err| {
                        KernelError::Manifest(format!(
                            "failed to decode plan input for {}: {err}",
                            binding.plan
                        ))
                    })?;
                    let input_schema =
                        self.schema_index
                            .get(plan_def.input.as_str())
                            .ok_or_else(|| {
                                KernelError::Manifest(format!(
                                    "plan '{}' input schema '{}' not found",
                                    plan_def.name, plan_def.input
                                ))
                            })?;
                    let input =
                        cbor_to_expr_value(&normalized.value, input_schema, &self.schema_index)?;
                    let correlation =
                        determine_correlation_value(binding, &input, event.key.as_ref());
                    let instance_id = self.scheduler.alloc_plan_id();
                    let cap_handles = self
                        .plan_cap_handles
                        .get(&plan_def.name)
                        .ok_or_else(|| {
                            KernelError::Manifest(format!(
                                "plan '{}' missing cap bindings",
                                plan_def.name
                            ))
                        })?
                        .clone();
                    let mut instance = PlanInstance::new(
                        instance_id,
                        plan_def.clone(),
                        input,
                        self.schema_index.clone(),
                        correlation,
                        cap_handles,
                    );
                    instance.set_context(crate::plan::PlanContext::from_stamp(stamp));
                    self.plan_instances.insert(instance_id, instance);
                    self.scheduler.push_plan(instance_id);
                }
            }
        }
        Ok(())
    }

    fn handle_plan_task(&mut self, id: u64) -> Result<(), KernelError> {
        let waiting_schema = self
            .plan_instances
            .get(&id)
            .and_then(|inst| inst.waiting_event_schema())
            .map(|s| s.to_string());
        if let Some(schema) = waiting_schema {
            self.remove_plan_from_waiting_events_for_schema(id, &schema);
        }
        if self.plan_instances.contains_key(&id) {
            let (plan_name, outcome, step_states) = {
                let instance = self
                    .plan_instances
                    .get_mut(&id)
                    .expect("instance must exist");
                let name = instance.name.clone();
                let snapshot = instance.snapshot();
                if let Some(context) = instance.context().cloned() {
                    self.effect_manager
                        .set_cap_context(crate::effects::CapContext {
                            logical_now_ns: context.logical_now_ns,
                            journal_height: context.journal_height,
                            manifest_hash: context.manifest_hash.clone(),
                        });
                } else {
                    self.effect_manager.clear_cap_context();
                }
                let outcome = match instance.tick(&mut self.effect_manager) {
                    Ok(outcome) => outcome,
                    Err(err) => {
                        self.record_decisions()?;
                        return Err(err);
                    }
                };
                (name, outcome, snapshot.step_states)
            };
            self.record_decisions()?;
            for event in &outcome.raised_events {
                self.process_domain_event(event.clone())?;
            }
            let mut intent_kinds = HashMap::new();
            for intent in &outcome.intents_enqueued {
                self.record_effect_intent(
                    intent,
                    IntentOriginRecord::Plan {
                        name: plan_name.clone(),
                        plan_id: id,
                    },
                )?;
                intent_kinds.insert(intent.intent_hash, intent.kind.as_str().to_string());
            }
            for hash in &outcome.waiting_receipts {
                let kind = intent_kinds.get(hash).cloned().or_else(|| {
                    self.effect_manager
                        .queued()
                        .iter()
                        .find(|intent| intent.intent_hash == *hash)
                        .map(|intent| intent.kind.as_str().to_string())
                });
                self.pending_receipts.insert(
                    *hash,
                    PendingPlanReceiptInfo {
                        plan_id: id,
                        effect_kind: kind.unwrap_or_else(|| "unknown".into()),
                    },
                );
            }
            if let Some(schema) = outcome.waiting_event.clone() {
                self.waiting_events.entry(schema).or_default().push(id);
            }
            if outcome.completed {
                if !self.suppress_journal {
                    let status = if outcome.plan_error.is_some() {
                        PlanEndStatus::Error
                    } else {
                        PlanEndStatus::Ok
                    };
                    let ended = PlanEndedRecord {
                        plan_name: plan_name.clone(),
                        plan_id: id,
                        status: status.clone(),
                        error_code: outcome.plan_error.as_ref().map(|err| err.code.clone()),
                    };
                    self.record_plan_ended(ended)?;
                    if status == PlanEndStatus::Ok {
                        if let (Some(value_cbor), Some(output_schema)) =
                            (outcome.result_cbor.clone(), outcome.result_schema.clone())
                        {
                            let entry = PlanResultEntry::new(
                                plan_name.clone(),
                                id,
                                output_schema,
                                value_cbor,
                            );
                            self.record_plan_result(&entry)?;
                            self.push_plan_result_entry(entry);
                        }
                    }
                }
                self.plan_instances.remove(&id);
            } else if outcome.waiting_receipts.is_empty() && outcome.waiting_event.is_none() {
                self.scheduler.push_plan(id);
            }
        }
        Ok(())
    }

    pub fn handle_receipt(
        &mut self,
        receipt: aos_effects::EffectReceipt,
    ) -> Result<(), KernelError> {
        let journal_height = self.journal.next_seq();
        let stamp = self.sample_ingress(journal_height)?;
        self.handle_receipt_with_ingress(receipt, stamp)
    }

    fn handle_receipt_with_ingress(
        &mut self,
        receipt: aos_effects::EffectReceipt,
        stamp: IngressStamp,
    ) -> Result<(), KernelError> {
        Self::validate_entropy(&stamp.entropy)?;
        if let Some(pending) = self.pending_receipts.remove(&receipt.intent_hash) {
            self.record_effect_receipt(&receipt, &stamp)?;
            self.record_decisions()?;
            if let Some(instance) = self.plan_instances.get_mut(&pending.plan_id) {
                instance.set_context(crate::plan::PlanContext::from_stamp(&stamp));
                if instance.deliver_receipt(receipt.intent_hash, &receipt.payload_cbor)? {
                    self.scheduler.push_plan(pending.plan_id);
                }
                self.remember_receipt(receipt.intent_hash);
                return Ok(());
            } else {
                log::warn!(
                    "receipt {} arrived for completed plan {}",
                    format_intent_hash(&receipt.intent_hash),
                    pending.plan_id
                );
                self.remember_receipt(receipt.intent_hash);
                return Ok(());
            }
        }

        if self.recent_receipt_index.contains(&receipt.intent_hash) {
            log::warn!(
                "late receipt {} ignored (already applied)",
                format_intent_hash(&receipt.intent_hash)
            );
            return Ok(());
        }

        if let Some(context) = self.pending_reducer_receipts.remove(&receipt.intent_hash) {
            self.record_effect_receipt(&receipt, &stamp)?;
            self.record_decisions()?;
            let event = build_reducer_receipt_event(&context, &receipt)?;
            self.process_domain_event(event)?;
            self.remember_receipt(receipt.intent_hash);
            return Ok(());
        }

        if self.suppress_journal {
            log::warn!(
                "receipt {} ignored during replay (no pending context)",
                format_intent_hash(&receipt.intent_hash)
            );
            return Ok(());
        }

        Err(KernelError::UnknownReceipt(format_intent_hash(
            &receipt.intent_hash,
        )))
    }

    fn deliver_event_to_waiting_plans(
        &mut self,
        event: &DomainEvent,
        stamp: &IngressStamp,
    ) -> Result<(), KernelError> {
        if let Some(mut plan_ids) = self.waiting_events.remove(&event.schema) {
            let mut still_waiting = Vec::new();
            for id in plan_ids.drain(..) {
                if let Some(instance) = self.plan_instances.get_mut(&id) {
                    instance.set_context(crate::plan::PlanContext::from_stamp(stamp));
                    if instance.deliver_event(event)? {
                        self.scheduler.push_plan(id);
                    } else {
                        still_waiting.push(id);
                    }
                }
            }
            if !still_waiting.is_empty() {
                self.waiting_events
                    .insert(event.schema.clone(), still_waiting);
            }
        }
        Ok(())
    }

    fn remove_plan_from_waiting_events_for_schema(&mut self, plan_id: u64, schema: &str) {
        if let Some(ids) = self.waiting_events.get_mut(schema) {
            if let Some(pos) = ids.iter().position(|id| *id == plan_id) {
                ids.swap_remove(pos);
            }
            if ids.is_empty() {
                self.waiting_events.remove(schema);
            }
        }
    }

    fn remember_receipt(&mut self, hash: [u8; 32]) {
        if self.recent_receipt_index.contains(&hash) {
            return;
        }
        if self.recent_receipts.len() >= RECENT_RECEIPT_CACHE {
            if let Some(old) = self.recent_receipts.pop_front() {
                self.recent_receipt_index.remove(&old);
            }
        }
        self.recent_receipts.push_back(hash);
        self.recent_receipt_index.insert(hash);
    }

    fn push_plan_result_entry(&mut self, entry: PlanResultEntry) {
        if self.plan_results.len() >= RECENT_PLAN_RESULT_CACHE {
            self.plan_results.pop_front();
        }
        self.plan_results.push_back(entry);
    }

    fn record_plan_result(&mut self, entry: &PlanResultEntry) -> Result<(), KernelError> {
        let record = entry.to_record();
        self.append_record(JournalRecord::PlanResult(record))
    }

    fn record_plan_ended(&mut self, record: PlanEndedRecord) -> Result<(), KernelError> {
        self.append_record(JournalRecord::PlanEnded(record))
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

fn decode_tail_record(kind: JournalKind, payload: &[u8]) -> Result<JournalRecord, KernelError> {
    if let Ok(record) = serde_cbor::from_slice::<JournalRecord>(payload) {
        return Ok(record);
    }
    // Backward-compatible fallback for older payloads that were encoded as raw records.
    let err = |e: serde_cbor::Error| KernelError::Journal(e.to_string());
    match kind {
        JournalKind::DomainEvent => serde_cbor::from_slice(payload)
            .map(JournalRecord::DomainEvent)
            .map_err(err),
        JournalKind::EffectIntent => serde_cbor::from_slice(payload)
            .map(JournalRecord::EffectIntent)
            .map_err(err),
        JournalKind::EffectReceipt => serde_cbor::from_slice(payload)
            .map(JournalRecord::EffectReceipt)
            .map_err(err),
        JournalKind::CapDecision => serde_cbor::from_slice(payload)
            .map(JournalRecord::CapDecision)
            .map_err(err),
        JournalKind::PolicyDecision => serde_cbor::from_slice(payload)
            .map(JournalRecord::PolicyDecision)
            .map_err(err),
        JournalKind::Manifest => serde_cbor::from_slice(payload)
            .map(JournalRecord::Manifest)
            .map_err(err),
        JournalKind::Snapshot => serde_cbor::from_slice(payload)
            .map(JournalRecord::Snapshot)
            .map_err(err),
        JournalKind::Governance => serde_cbor::from_slice(payload)
            .map(JournalRecord::Governance)
            .map_err(err),
        JournalKind::PlanResult => serde_cbor::from_slice(payload)
            .map(JournalRecord::PlanResult)
            .map_err(err),
        JournalKind::PlanEnded => serde_cbor::from_slice(payload)
            .map(JournalRecord::PlanEnded)
            .map_err(err),
        JournalKind::Custom => serde_cbor::from_slice(payload)
            .map(JournalRecord::Custom)
            .map_err(err),
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

fn extract_cbor_path(value: &CborValue, path: &str) -> Option<CborValue> {
    let mut current = value;
    for segment in path.split('.') {
        if segment.is_empty() {
            continue;
        }
        current = match current {
            CborValue::Map(map) => map.get(&CborValue::Text(segment.to_string()))?,
            _ => return None,
        };
    }
    Some(current.clone())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{JournalEntry, JournalKind, mem::MemJournal};
    use aos_air_types::{
        CURRENT_AIR_VERSION, DefSchema, HashRef, ModuleAbi, ModuleKind, ReducerAbi, Routing,
        RoutingEvent, SchemaRef, SecretDecl, TypeExpr, TypePrimitive, TypePrimitiveHash,
        TypePrimitiveText, TypeRecord,
    };
    use aos_cbor::to_canonical_cbor;
    use aos_store::MemStore;
    use aos_wasm_abi::ReducerEffect;
    use indexmap::IndexMap;
    use serde_cbor::ser::to_vec;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::fs::File;
    use std::io::Write;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn named_ref(name: &str, hash: &str) -> NamedRef {
        NamedRef {
            name: name.into(),
            hash: HashRef::new(hash).unwrap(),
        }
    }

    fn hash(num: u64) -> String {
        // Produce a valid sha256: prefixed hex string for tests
        format!("sha256:{num:064x}")
    }

    fn minimal_manifest() -> Manifest {
        Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: vec![],
            modules: vec![],
            plans: vec![],
            caps: vec![],
            effects: vec![],
            policies: vec![],
            secrets: vec![],
            triggers: vec![],
            module_bindings: IndexMap::new(),
            routing: None,
            defaults: None,
        }
    }

    fn dummy_stamp<S: Store + 'static>(kernel: &Kernel<S>) -> IngressStamp {
        IngressStamp {
            now_ns: 0,
            logical_now_ns: 0,
            entropy: vec![0u8; ENTROPY_LEN],
            journal_height: 0,
            manifest_hash: kernel.manifest_hash().to_hex(),
        }
    }

    #[test]
    fn route_event_requires_key_for_keyed_reducer() {
        let kernel = minimal_kernel_keyed_missing_key_field();
        let payload = serde_cbor::to_vec(&CborValue::Map(BTreeMap::from([(
            CborValue::Text("id".into()),
            CborValue::Text("1".into()),
        )])))
        .unwrap();
        let event = DomainEvent::new("com.acme/Event@1", payload);
        let err = kernel
            .route_event(&event, &dummy_stamp(&kernel))
            .unwrap_err();
        assert!(format!("{err:?}").contains("missing key_field"), "{err}");
    }

    #[test]
    fn route_event_rejects_key_for_non_keyed_reducer() {
        let kernel = minimal_kernel_with_router_non_keyed();
        let payload = serde_cbor::to_vec(&CborValue::Map(BTreeMap::from([(
            CborValue::Text("id".into()),
            CborValue::Text("1".into()),
        )])))
        .unwrap();
        let event = DomainEvent::new("com.acme/Event@1", payload);
        let err = kernel
            .route_event(&event, &dummy_stamp(&kernel))
            .unwrap_err();
        assert!(format!("{err:?}").contains("provided key_field"), "{err}");
    }

    #[test]
    fn route_event_extracts_key_and_passes_to_reducer() {
        let kernel = minimal_kernel_with_router();
        let payload = serde_cbor::to_vec(&CborValue::Map(BTreeMap::from([(
            CborValue::Text("id".into()),
            CborValue::Text("abc".into()),
        )])))
        .unwrap();
        let event = DomainEvent::new("com.acme/Event@1", payload);
        let routed = kernel
            .route_event(&event, &dummy_stamp(&kernel))
            .expect("route");
        assert_eq!(routed.len(), 1);
        let expected_key = aos_cbor::to_canonical_cbor(&CborValue::Text("abc".into())).unwrap();
        assert_eq!(routed[0].event.key.as_ref().unwrap(), &expected_key);
        assert_eq!(routed[0].reducer, "com.acme/Reducer@1");
    }

    #[test]
    fn event_normalization_rejects_invalid_payload() {
        let mut kernel = minimal_kernel_with_router();
        let payload = serde_cbor::to_vec(&CborValue::Integer(5.into())).unwrap();
        let err = kernel
            .submit_domain_event_result("com.acme/Event@1", payload)
            .unwrap_err();
        assert!(
            matches!(err, KernelError::Manifest(msg) if msg.contains("payload failed validation"))
        );
    }

    #[test]
    fn replay_applies_manifest_records_in_order() {
        let store = Arc::new(MemStore::default());
        let (loaded_a, hash_a) = loaded_manifest_with_schema(store.as_ref(), "com.acme/EventA@1");
        let (_loaded_b, hash_b) = loaded_manifest_with_schema(store.as_ref(), "com.acme/EventB@1");

        let mut journal = MemJournal::default();
        append_record(
            &mut journal,
            JournalRecord::Manifest(ManifestRecord {
                manifest_hash: hash_a.to_hex(),
            }),
        );
        append_record(
            &mut journal,
            JournalRecord::DomainEvent(event_record("com.acme/EventA@1", 1)),
        );
        append_record(
            &mut journal,
            JournalRecord::Manifest(ManifestRecord {
                manifest_hash: hash_b.to_hex(),
            }),
        );
        append_record(
            &mut journal,
            JournalRecord::DomainEvent(event_record("com.acme/EventB@1", 3)),
        );

        let kernel = Kernel::from_loaded_manifest_with_config(
            store,
            loaded_a,
            Box::new(journal),
            KernelConfig::default(),
        )
        .expect("replay");
        assert_eq!(kernel.manifest_hash().to_hex(), hash_b.to_hex());
    }

    #[test]
    fn replay_applies_manifest_after_snapshot() {
        let store = Arc::new(MemStore::default());
        let (loaded_a, hash_a) = loaded_manifest_with_schema(store.as_ref(), "com.acme/EventA@1");
        let (_loaded_b, hash_b) = loaded_manifest_with_schema(store.as_ref(), "com.acme/EventB@1");

        let mut journal = MemJournal::default();
        append_record(
            &mut journal,
            JournalRecord::Manifest(ManifestRecord {
                manifest_hash: hash_a.to_hex(),
            }),
        );
        append_record(
            &mut journal,
            JournalRecord::DomainEvent(event_record("com.acme/EventA@1", 1)),
        );

        let snapshot_height = 2;
        let mut snapshot = KernelSnapshot::new(
            snapshot_height,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            0,
            vec![],
            vec![],
            vec![],
            0,
            Some(*hash_a.as_bytes()),
        );
        snapshot.set_root_completeness(SnapshotRootCompleteness {
            manifest_hash: Some(hash_a.as_bytes().to_vec()),
            ..SnapshotRootCompleteness::default()
        });
        let snap_bytes = serde_cbor::to_vec(&snapshot).expect("encode snapshot");
        let snap_hash = store.put_blob(&snap_bytes).expect("store snapshot");
        append_record(
            &mut journal,
            JournalRecord::Snapshot(SnapshotRecord {
                snapshot_ref: snap_hash.to_hex(),
                height: snapshot_height,
                logical_time_ns: 0,
                receipt_horizon_height: Some(snapshot_height),
                manifest_hash: Some(hash_a.to_hex()),
            }),
        );

        append_record(
            &mut journal,
            JournalRecord::DomainEvent(event_record("com.acme/EventA@1", 3)),
        );
        append_record(
            &mut journal,
            JournalRecord::Manifest(ManifestRecord {
                manifest_hash: hash_b.to_hex(),
            }),
        );
        append_record(
            &mut journal,
            JournalRecord::DomainEvent(event_record("com.acme/EventB@1", 5)),
        );

        let kernel = Kernel::from_loaded_manifest_with_config(
            store,
            loaded_a,
            Box::new(journal),
            KernelConfig::default(),
        )
        .expect("replay");
        assert_eq!(kernel.manifest_hash().to_hex(), hash_b.to_hex());
    }

    #[test]
    fn world_initialization_persists_active_baseline() {
        let kernel = minimal_kernel_non_keyed();
        let heights = kernel.heights();
        assert!(
            heights.snapshot.is_some(),
            "new worlds should always have an active baseline"
        );
        let entries = kernel.dump_journal().expect("journal");
        let baseline = entries
            .iter()
            .find_map(
                |entry| match serde_cbor::from_slice::<JournalRecord>(&entry.payload).ok() {
                    Some(JournalRecord::Snapshot(record)) => Some(record),
                    _ => None,
                },
            )
            .expect("baseline snapshot record");
        assert!(
            baseline.receipt_horizon_height.is_some(),
            "initial baseline should carry a receipt horizon when no pending receipts exist"
        );
    }

    #[test]
    fn unsafe_baseline_promotion_fails_receipt_horizon_precondition() {
        let mut kernel = minimal_kernel_non_keyed();
        let initial_baseline_height = kernel
            .active_baseline
            .as_ref()
            .expect("initial baseline")
            .height;

        kernel.pending_receipts.insert(
            [9u8; 32],
            PendingPlanReceiptInfo {
                plan_id: 42,
                effect_kind: "http.request".into(),
            },
        );
        kernel.create_snapshot().expect("snapshot still written");

        let active_height = kernel
            .active_baseline
            .as_ref()
            .expect("active baseline retained")
            .height;
        assert_eq!(
            active_height, initial_baseline_height,
            "unsafe snapshot must not promote active baseline"
        );
    }

    #[test]
    fn snapshot_root_completeness_missing_required_root_fails_closed() {
        let store = Arc::new(MemStore::default());
        let (loaded, manifest_hash) =
            loaded_manifest_with_schema(store.as_ref(), "com.acme/Event@1");

        let snapshot = KernelSnapshot::new(
            1,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            0,
            vec![],
            vec![],
            vec![],
            0,
            None, // missing manifest root on purpose
        );
        let snap_bytes = serde_cbor::to_vec(&snapshot).unwrap();
        let snap_hash = store.put_blob(&snap_bytes).unwrap();

        let mut bad_journal = MemJournal::default();
        append_record(
            &mut bad_journal,
            JournalRecord::Manifest(ManifestRecord {
                manifest_hash: manifest_hash.to_hex(),
            }),
        );
        append_record(
            &mut bad_journal,
            JournalRecord::Snapshot(SnapshotRecord {
                snapshot_ref: snap_hash.to_hex(),
                height: 1,
                logical_time_ns: 0,
                receipt_horizon_height: Some(1),
                manifest_hash: Some(manifest_hash.to_hex()),
            }),
        );

        let err = match Kernel::from_loaded_manifest(store, loaded, Box::new(bad_journal)) {
            Ok(_) => panic!("incomplete roots should fail"),
            Err(err) => err,
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("root completeness") || rendered.contains("snapshot"),
            "unexpected error: {rendered}"
        );
    }

    #[test]
    fn reducer_state_traversal_collects_only_typed_hash_refs() {
        let store = aos_store::MemStore::default();
        let module = DefModule {
            name: "com.acme/Reducer@1".into(),
            module_kind: ModuleKind::Reducer,
            wasm_hash: HashRef::new(hash(1)).unwrap(),
            key_schema: None,
            abi: ModuleAbi {
                reducer: Some(ReducerAbi {
                    state: SchemaRef::new("com.acme/StateRefs@1").unwrap(),
                    event: SchemaRef::new("com.acme/Event@1").unwrap(),
                    context: Some(SchemaRef::new("sys/ReducerContext@1").unwrap()),
                    annotations: None,
                    effects_emitted: vec![],
                    cap_slots: Default::default(),
                }),
                pure: None,
            },
        };
        let mut modules = HashMap::new();
        modules.insert(module.name.clone(), module);
        let mut schemas = HashMap::new();
        schemas.insert(
            "com.acme/StateRefs@1".into(),
            DefSchema {
                name: "com.acme/StateRefs@1".into(),
                ty: TypeExpr::Record(aos_air_types::TypeRecord {
                    record: IndexMap::from([
                        (
                            "direct".into(),
                            TypeExpr::Primitive(TypePrimitive::Hash(TypePrimitiveHash {
                                hash: Default::default(),
                            })),
                        ),
                        (
                            "nested".into(),
                            TypeExpr::List(aos_air_types::TypeList {
                                list: Box::new(TypeExpr::Primitive(TypePrimitive::Hash(
                                    TypePrimitiveHash {
                                        hash: Default::default(),
                                    },
                                ))),
                            }),
                        ),
                        (
                            "opaque_text".into(),
                            TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                                text: Default::default(),
                            })),
                        ),
                    ]),
                }),
            },
        );
        schemas.insert(
            "com.acme/Event@1".into(),
            schema_event_record("com.acme/Event@1"),
        );
        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: vec![],
            modules: vec![NamedRef {
                name: "com.acme/Reducer@1".into(),
                hash: HashRef::new(hash(1)).unwrap(),
            }],
            plans: vec![],
            effects: vec![],
            caps: vec![],
            policies: vec![],
            secrets: vec![],
            defaults: None,
            module_bindings: Default::default(),
            routing: Some(Routing {
                events: vec![RoutingEvent {
                    event: SchemaRef::new("com.acme/Event@1").unwrap(),
                    reducer: "com.acme/Reducer@1".to_string(),
                    key_field: None,
                }],
                inboxes: vec![],
            }),
            triggers: vec![],
        };
        let loaded = LoadedManifest {
            manifest,
            secrets: vec![],
            modules,
            plans: HashMap::new(),
            effects: HashMap::new(),
            caps: HashMap::new(),
            policies: HashMap::new(),
            schemas,
            effect_catalog: EffectCatalog::from_defs(Vec::new()),
        };
        let mut kernel = Kernel::from_loaded_manifest(
            Arc::new(store),
            loaded,
            Box::new(crate::journal::mem::MemJournal::default()),
        )
        .unwrap();

        let direct = hash(10);
        let nested = hash(11);
        let opaque = hash(12);
        let state = CborValue::Map(BTreeMap::from([
            (
                CborValue::Text("direct".into()),
                CborValue::Text(direct.clone()),
            ),
            (
                CborValue::Text("nested".into()),
                CborValue::Array(vec![CborValue::Text(nested.clone())]),
            ),
            (
                CborValue::Text("opaque_text".into()),
                CborValue::Text(opaque.clone()),
            ),
        ]));
        kernel
            .handle_reducer_output(
                "com.acme/Reducer@1".into(),
                None,
                false,
                ReducerOutput {
                    state: Some(serde_cbor::to_vec(&state).unwrap()),
                    domain_events: vec![],
                    effects: vec![],
                    ann: None,
                },
            )
            .unwrap();

        let refs = kernel
            .reducer_state_typed_hash_refs("com.acme/Reducer@1", None)
            .unwrap();
        assert!(refs.contains(&Hash::from_hex_str(&direct).unwrap()));
        assert!(refs.contains(&Hash::from_hex_str(&nested).unwrap()));
        assert!(
            !refs.contains(&Hash::from_hex_str(&opaque).unwrap()),
            "opaque text hashes must not be auto-traversed"
        );
    }

    #[test]
    fn baseline_plus_tail_replay_matches_full_replay_state() {
        let store_full = Arc::new(MemStore::default());
        let (loaded_full, _) =
            loaded_manifest_with_schema(store_full.as_ref(), "com.acme/EventA@1");
        let mut kernel_full = Kernel::from_loaded_manifest(
            store_full.clone(),
            loaded_full,
            Box::new(MemJournal::default()),
        )
        .unwrap();
        kernel_full
            .submit_domain_event_result(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "1" })).unwrap(),
            )
            .unwrap();
        kernel_full
            .submit_domain_event_result(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "2" })).unwrap(),
            )
            .unwrap();
        kernel_full
            .submit_domain_event_result(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "3" })).unwrap(),
            )
            .unwrap();
        kernel_full.create_snapshot().unwrap();

        let store_baseline = Arc::new(MemStore::default());
        let (loaded_baseline, _) =
            loaded_manifest_with_schema(store_baseline.as_ref(), "com.acme/EventA@1");
        let mut kernel_baseline = Kernel::from_loaded_manifest(
            store_baseline.clone(),
            loaded_baseline,
            Box::new(MemJournal::default()),
        )
        .unwrap();
        kernel_baseline
            .submit_domain_event_result(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "1" })).unwrap(),
            )
            .unwrap();
        kernel_baseline.create_snapshot().unwrap();
        kernel_baseline
            .submit_domain_event_result(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "2" })).unwrap(),
            )
            .unwrap();
        kernel_baseline
            .submit_domain_event_result(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "3" })).unwrap(),
            )
            .unwrap();
        kernel_baseline.create_snapshot().unwrap();

        assert_eq!(
            kernel_full.manifest_hash, kernel_baseline.manifest_hash,
            "manifest hash must be identical after baseline+tail and full replay"
        );
        assert_eq!(
            kernel_full.reducer_index_roots, kernel_baseline.reducer_index_roots,
            "cell index roots must be identical after baseline+tail and full replay"
        );
    }

    #[test]
    fn non_keyed_state_persisted_via_cell_index() {
        let mut kernel = minimal_kernel_non_keyed();
        let reducer = "com.acme/Reducer@1".to_string();
        let state_bytes = b"non-keyed-state".to_vec();
        let output = ReducerOutput {
            state: Some(state_bytes.clone()),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        };

        kernel
            .handle_reducer_output(reducer.clone(), None, false, output)
            .expect("write state");

        let root = kernel.reducer_index_root(&reducer);
        assert!(root.is_some(), "expected index root for non-keyed reducer");
        let cells = kernel.list_cells(&reducer).expect("list cells");
        assert_eq!(cells.len(), 1, "expected sentinel cell entry");
        assert!(
            cells[0].key_bytes.is_empty(),
            "sentinel key should be empty"
        );

        // Evict from cache and reload via index/CAS.
        if let Some(entry) = kernel.reducer_state.get_mut(&reducer) {
            entry.cell_cache.remove(MONO_KEY);
        }
        let reloaded = kernel
            .reducer_state_bytes(&reducer, None)
            .expect("read state")
            .expect("state present");
        assert_eq!(reloaded, state_bytes);
    }

    fn schema_text(name: &str) -> DefSchema {
        DefSchema {
            name: name.into(),
            ty: TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                text: Default::default(),
            })),
        }
    }

    fn schema_event_record(name: &str) -> DefSchema {
        DefSchema {
            name: name.into(),
            ty: TypeExpr::Record(TypeRecord {
                record: IndexMap::from([(
                    "id".into(),
                    TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                        text: Default::default(),
                    })),
                )]),
            }),
        }
    }

    fn loaded_manifest_with_schema(
        store: &MemStore,
        schema_name: &str,
    ) -> (LoadedManifest, aos_cbor::Hash) {
        let schema = schema_event_record(schema_name);
        let schema_hash = store
            .put_node(&AirNode::Defschema(schema.clone()))
            .expect("store schema");
        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: vec![NamedRef {
                name: schema_name.into(),
                hash: HashRef::new(schema_hash.to_hex()).unwrap(),
            }],
            modules: vec![],
            plans: vec![],
            caps: vec![],
            effects: vec![],
            policies: vec![],
            secrets: vec![],
            triggers: vec![],
            module_bindings: Default::default(),
            routing: None,
            defaults: None,
        };
        let loaded = LoadedManifest {
            manifest,
            secrets: vec![],
            modules: HashMap::new(),
            plans: HashMap::new(),
            effects: HashMap::new(),
            caps: HashMap::new(),
            policies: HashMap::new(),
            schemas: HashMap::from([(schema_name.into(), schema)]),
            effect_catalog: EffectCatalog::from_defs(Vec::new()),
        };
        let mut loaded = loaded;
        manifest_runtime::persist_loaded_manifest(store, &mut loaded).expect("persist manifest");
        let manifest_hash = store.put_node(&loaded.manifest).expect("store manifest");
        (loaded, manifest_hash)
    }

    fn event_record(schema: &str, journal_height: u64) -> DomainEventRecord {
        let payload = serde_cbor::to_vec(&CborValue::Map(BTreeMap::from([(
            CborValue::Text("id".into()),
            CborValue::Text("1".into()),
        )])))
        .unwrap();
        DomainEventRecord {
            schema: schema.to_string(),
            value: payload,
            key: None,
            now_ns: 0,
            logical_now_ns: 0,
            journal_height,
            entropy: vec![0u8; ENTROPY_LEN],
            event_hash: String::new(),
            manifest_hash: String::new(),
        }
    }

    fn append_record(journal: &mut MemJournal, record: JournalRecord) {
        let bytes = serde_cbor::to_vec(&record).expect("encode record");
        journal
            .append(JournalEntry::new(record.kind(), &bytes))
            .expect("append record");
    }

    fn minimal_kernel_with_router() -> Kernel<aos_store::MemStore> {
        let store = aos_store::MemStore::default();
        let module = DefModule {
            name: "com.acme/Reducer@1".into(),
            module_kind: ModuleKind::Reducer,
            wasm_hash: HashRef::new(hash(1)).unwrap(),
            key_schema: Some(SchemaRef::new("com.acme/Key@1").unwrap()),
            abi: ModuleAbi {
                reducer: Some(ReducerAbi {
                    state: SchemaRef::new("com.acme/State@1").unwrap(),
                    event: SchemaRef::new("com.acme/Event@1").unwrap(),
                    context: Some(SchemaRef::new("sys/ReducerContext@1").unwrap()),
                    annotations: None,
                    effects_emitted: vec![],
                    cap_slots: Default::default(),
                }),
                pure: None,
            },
        };
        let mut modules = HashMap::new();
        modules.insert(module.name.clone(), module);
        let mut schemas = HashMap::new();
        schemas.insert("com.acme/State@1".into(), schema_text("com.acme/State@1"));
        schemas.insert(
            "com.acme/Event@1".into(),
            schema_event_record("com.acme/Event@1"),
        );
        schemas.insert("com.acme/Key@1".into(), schema_text("com.acme/Key@1"));
        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: vec![],
            modules: vec![NamedRef {
                name: "com.acme/Reducer@1".into(),
                hash: HashRef::new(hash(1)).unwrap(),
            }],
            plans: vec![],
            effects: vec![],
            caps: vec![],
            policies: vec![],
            secrets: vec![],
            defaults: None,
            module_bindings: Default::default(),
            routing: Some(Routing {
                events: vec![RoutingEvent {
                    event: SchemaRef::new("com.acme/Event@1").unwrap(),
                    reducer: "com.acme/Reducer@1".to_string(),
                    key_field: Some("id".into()),
                }],
                inboxes: vec![],
            }),
            triggers: vec![],
        };
        let loaded = LoadedManifest {
            manifest,
            secrets: vec![],
            modules,
            plans: HashMap::new(),
            effects: HashMap::new(),
            caps: HashMap::new(),
            policies: HashMap::new(),
            schemas,
            effect_catalog: EffectCatalog::from_defs(Vec::new()),
        };
        Kernel::from_loaded_manifest(
            Arc::new(store),
            loaded,
            Box::new(crate::journal::mem::MemJournal::default()),
        )
        .unwrap()
    }

    fn minimal_kernel_with_router_non_keyed() -> Kernel<aos_store::MemStore> {
        let store = aos_store::MemStore::default();
        let module = DefModule {
            name: "com.acme/Reducer@1".into(),
            module_kind: ModuleKind::Reducer,
            wasm_hash: HashRef::new(hash(1)).unwrap(),
            key_schema: None,
            abi: ModuleAbi {
                reducer: Some(ReducerAbi {
                    state: SchemaRef::new("com.acme/State@1").unwrap(),
                    event: SchemaRef::new("com.acme/Event@1").unwrap(),
                    context: Some(SchemaRef::new("sys/ReducerContext@1").unwrap()),
                    annotations: None,
                    effects_emitted: vec![],
                    cap_slots: Default::default(),
                }),
                pure: None,
            },
        };
        let mut modules = HashMap::new();
        modules.insert(module.name.clone(), module);
        let mut schemas = HashMap::new();
        schemas.insert("com.acme/State@1".into(), schema_text("com.acme/State@1"));
        schemas.insert(
            "com.acme/Event@1".into(),
            schema_event_record("com.acme/Event@1"),
        );
        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: vec![],
            modules: vec![NamedRef {
                name: "com.acme/Reducer@1".into(),
                hash: HashRef::new(hash(1)).unwrap(),
            }],
            plans: vec![],
            effects: vec![],
            caps: vec![],
            policies: vec![],
            secrets: vec![],
            defaults: None,
            module_bindings: Default::default(),
            routing: Some(Routing {
                events: vec![RoutingEvent {
                    event: SchemaRef::new("com.acme/Event@1").unwrap(),
                    reducer: "com.acme/Reducer@1".to_string(),
                    key_field: Some("id".into()),
                }],
                inboxes: vec![],
            }),
            triggers: vec![],
        };
        let loaded = LoadedManifest {
            manifest,
            secrets: vec![],
            modules,
            plans: HashMap::new(),
            effects: HashMap::new(),
            caps: HashMap::new(),
            policies: HashMap::new(),
            schemas,
            effect_catalog: EffectCatalog::from_defs(Vec::new()),
        };
        Kernel::from_loaded_manifest(
            Arc::new(store),
            loaded,
            Box::new(crate::journal::mem::MemJournal::default()),
        )
        .unwrap()
    }

    fn minimal_kernel_non_keyed() -> Kernel<aos_store::MemStore> {
        let store = aos_store::MemStore::default();
        let module = DefModule {
            name: "com.acme/Reducer@1".into(),
            module_kind: ModuleKind::Reducer,
            wasm_hash: HashRef::new(hash(1)).unwrap(),
            key_schema: None,
            abi: ModuleAbi {
                reducer: Some(ReducerAbi {
                    state: SchemaRef::new("com.acme/State@1").unwrap(),
                    event: SchemaRef::new("com.acme/Event@1").unwrap(),
                    context: Some(SchemaRef::new("sys/ReducerContext@1").unwrap()),
                    annotations: None,
                    effects_emitted: vec![],
                    cap_slots: Default::default(),
                }),
                pure: None,
            },
        };
        let mut modules = HashMap::new();
        modules.insert(module.name.clone(), module);
        let mut schemas = HashMap::new();
        schemas.insert("com.acme/State@1".into(), schema_text("com.acme/State@1"));
        schemas.insert(
            "com.acme/Event@1".into(),
            schema_event_record("com.acme/Event@1"),
        );
        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: vec![],
            modules: vec![NamedRef {
                name: "com.acme/Reducer@1".into(),
                hash: HashRef::new(hash(1)).unwrap(),
            }],
            plans: vec![],
            effects: vec![],
            caps: vec![],
            policies: vec![],
            secrets: vec![],
            defaults: None,
            module_bindings: Default::default(),
            routing: Some(Routing {
                events: vec![RoutingEvent {
                    event: SchemaRef::new("com.acme/Event@1").unwrap(),
                    reducer: "com.acme/Reducer@1".to_string(),
                    key_field: None,
                }],
                inboxes: vec![],
            }),
            triggers: vec![],
        };
        let loaded = LoadedManifest {
            manifest,
            secrets: vec![],
            modules,
            plans: HashMap::new(),
            effects: HashMap::new(),
            caps: HashMap::new(),
            policies: HashMap::new(),
            schemas,
            effect_catalog: EffectCatalog::from_defs(Vec::new()),
        };
        Kernel::from_loaded_manifest(
            Arc::new(store),
            loaded,
            Box::new(crate::journal::mem::MemJournal::default()),
        )
        .unwrap()
    }

    fn minimal_kernel_keyed_missing_key_field() -> Kernel<aos_store::MemStore> {
        let store = aos_store::MemStore::default();
        let module = DefModule {
            name: "com.acme/Reducer@1".into(),
            module_kind: ModuleKind::Reducer,
            wasm_hash: HashRef::new(hash(1)).unwrap(),
            key_schema: Some(SchemaRef::new("com.acme/Key@1").unwrap()),
            abi: ModuleAbi {
                reducer: Some(ReducerAbi {
                    state: SchemaRef::new("com.acme/State@1").unwrap(),
                    event: SchemaRef::new("com.acme/Event@1").unwrap(),
                    context: Some(SchemaRef::new("sys/ReducerContext@1").unwrap()),
                    annotations: None,
                    effects_emitted: vec![],
                    cap_slots: Default::default(),
                }),
                pure: None,
            },
        };
        let mut modules = HashMap::new();
        modules.insert(module.name.clone(), module);
        let mut schemas = HashMap::new();
        schemas.insert("com.acme/State@1".into(), schema_text("com.acme/State@1"));
        schemas.insert(
            "com.acme/Event@1".into(),
            schema_event_record("com.acme/Event@1"),
        );
        schemas.insert("com.acme/Key@1".into(), schema_text("com.acme/Key@1"));
        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: vec![],
            modules: vec![NamedRef {
                name: "com.acme/Reducer@1".into(),
                hash: HashRef::new(hash(1)).unwrap(),
            }],
            plans: vec![],
            effects: vec![],
            caps: vec![],
            policies: vec![],
            secrets: vec![],
            defaults: None,
            module_bindings: Default::default(),
            routing: Some(Routing {
                events: vec![RoutingEvent {
                    event: SchemaRef::new("com.acme/Event@1").unwrap(),
                    reducer: "com.acme/Reducer@1".to_string(),
                    key_field: None,
                }],
                inboxes: vec![],
            }),
            triggers: vec![],
        };
        let loaded = LoadedManifest {
            manifest,
            secrets: vec![],
            modules,
            plans: HashMap::new(),
            effects: HashMap::new(),
            caps: HashMap::new(),
            policies: HashMap::new(),
            schemas,
            effect_catalog: EffectCatalog::from_defs(Vec::new()),
        };
        Kernel::from_loaded_manifest(
            Arc::new(store),
            loaded,
            Box::new(crate::journal::mem::MemJournal::default()),
        )
        .unwrap()
    }

    fn empty_manifest() -> Manifest {
        Manifest {
            air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
            schemas: vec![],
            modules: vec![],
            plans: vec![],
            effects: vec![],
            caps: vec![],
            policies: vec![],
            secrets: vec![],
            defaults: None,
            module_bindings: Default::default(),
            routing: None,
            triggers: vec![],
        }
    }

    fn write_manifest(path: &std::path::Path, manifest: &Manifest) {
        let bytes = to_vec(manifest).expect("serialize manifest");
        let mut file = File::create(path).expect("create manifest file");
        file.write_all(&bytes).expect("write manifest");
    }

    #[test]
    fn reducer_output_with_multiple_effects_is_rejected() {
        let output = ReducerOutput {
            effects: vec![
                ReducerEffect::new("timer.set", vec![1]),
                ReducerEffect::new("blob.put", vec![2]),
            ],
            ..Default::default()
        };

        let err = Kernel::<MemStore>::ensure_single_effect(&output).unwrap_err();
        assert!(
            matches!(err, KernelError::ReducerOutput(ref message) if message.contains("at most one effect")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn ledger_deltas_capture_added_changed_and_removed() {
        let current = Manifest {
            caps: vec![
                named_ref("cap/a@1", &hash(1)),
                named_ref("cap/b@1", &hash(2)),
            ],
            policies: vec![named_ref("policy/old@1", &hash(3))],
            ..empty_manifest()
        };
        let candidate = Manifest {
            caps: vec![
                named_ref("cap/a@1", &hash(99)),
                named_ref("cap/c@1", &hash(4)),
            ],
            policies: vec![named_ref("policy/new@1", &hash(5))],
            ..empty_manifest()
        };

        let deltas = Kernel::<MemStore>::compute_ledger_deltas(&current, &candidate);

        assert_eq!(deltas.len(), 5);
        assert!(deltas.contains(&LedgerDelta {
            ledger: LedgerKind::Capability,
            name: "cap/a@1".to_string(),
            change: DeltaKind::Changed,
        }));
        assert!(deltas.contains(&LedgerDelta {
            ledger: LedgerKind::Capability,
            name: "cap/c@1".to_string(),
            change: DeltaKind::Added,
        }));
        assert!(deltas.contains(&LedgerDelta {
            ledger: LedgerKind::Capability,
            name: "cap/b@1".to_string(),
            change: DeltaKind::Removed,
        }));
        assert!(deltas.contains(&LedgerDelta {
            ledger: LedgerKind::Policy,
            name: "policy/old@1".to_string(),
            change: DeltaKind::Removed,
        }));
        assert!(deltas.contains(&LedgerDelta {
            ledger: LedgerKind::Policy,
            name: "policy/new@1".to_string(),
            change: DeltaKind::Added,
        }));
    }

    fn kernel_with_store_and_journal(
        store: Arc<MemStore>,
        journal: Box<dyn Journal>,
    ) -> Kernel<MemStore> {
        let manifest = empty_manifest();
        let loaded = LoadedManifest {
            manifest,
            secrets: vec![],
            modules: HashMap::new(),
            plans: HashMap::new(),
            effects: HashMap::new(),
            caps: HashMap::new(),
            policies: HashMap::new(),
            schemas: HashMap::new(),
            effect_catalog: EffectCatalog::from_defs(Vec::new()),
        };
        Kernel::from_loaded_manifest_with_config(store, loaded, journal, KernelConfig::default())
            .expect("build kernel")
    }

    #[test]
    fn cell_index_root_updates_on_upsert_and_delete() {
        let store = Arc::new(MemStore::default());
        let journal = Box::new(MemJournal::new());
        let mut kernel = kernel_with_store_and_journal(store.clone(), journal);
        let reducer = "com.acme/Reducer@1".to_string();
        let key = b"abc".to_vec();

        // initial insert
        kernel
            .handle_reducer_output(
                reducer.clone(),
                Some(key.clone()),
                true,
                ReducerOutput {
                    state: Some(vec![1]),
                    ..Default::default()
                },
            )
            .unwrap();
        let root1 = *kernel.reducer_index_roots.get(&reducer).unwrap();
        let index = CellIndex::new(store.as_ref());
        let meta1 = index
            .get(root1, Hash::of_bytes(&key).as_bytes())
            .unwrap()
            .expect("meta1");
        assert_eq!(meta1.state_hash, *Hash::of_bytes(&[1]).as_bytes());

        // update same key
        kernel
            .handle_reducer_output(
                reducer.clone(),
                Some(key.clone()),
                true,
                ReducerOutput {
                    state: Some(vec![2]),
                    ..Default::default()
                },
            )
            .unwrap();
        let root2 = *kernel.reducer_index_roots.get(&reducer).unwrap();
        assert_ne!(root1, root2);
        let meta2 = index
            .get(root2, Hash::of_bytes(&key).as_bytes())
            .unwrap()
            .expect("meta2");
        assert_eq!(meta2.state_hash, *Hash::of_bytes(&[2]).as_bytes());

        // delete
        kernel
            .handle_reducer_output(
                reducer.clone(),
                Some(key.clone()),
                true,
                ReducerOutput {
                    state: None,
                    ..Default::default()
                },
            )
            .unwrap();
        let root3 = *kernel.reducer_index_roots.get(&reducer).unwrap();
        assert_ne!(root2, root3);
        let meta3 = index.get(root3, Hash::of_bytes(&key).as_bytes()).unwrap();
        assert!(meta3.is_none());
    }

    #[test]
    fn snapshot_restores_cell_index_root_and_cells() {
        let store = Arc::new(MemStore::default());
        let journal = Box::new(MemJournal::new());
        let mut kernel = kernel_with_store_and_journal(store.clone(), journal);
        let reducer = "com.acme/Reducer@1".to_string();
        let key = b"k".to_vec();
        let state_bytes = vec![9u8, 9u8];

        kernel
            .handle_reducer_output(
                reducer.clone(),
                Some(key.clone()),
                true,
                ReducerOutput {
                    state: Some(state_bytes.clone()),
                    ..Default::default()
                },
            )
            .unwrap();
        let root_before = *kernel.reducer_index_roots.get(&reducer).unwrap();

        kernel.create_snapshot().unwrap();
        let entries = kernel.journal.load_from(0).expect("load journal entries");

        // Rehydrate kernel from snapshot + shared store.
        let mut kernel2 = {
            let journal = Box::new(MemJournal::from_entries(&entries));
            kernel_with_store_and_journal(store.clone(), journal)
        };
        kernel2.tick_until_idle().unwrap();

        let root_after = *kernel2.reducer_index_roots.get(&reducer).unwrap();
        assert_eq!(root_before, root_after);

        let index = CellIndex::new(store.as_ref());
        let meta = index
            .get(root_after, Hash::of_bytes(&key).as_bytes())
            .unwrap()
            .expect("restored meta");
        assert_eq!(meta.state_hash, *Hash::of_bytes(&state_bytes).as_bytes());
        let restored_state = store
            .get_blob(Hash::from_bytes(&meta.state_hash).unwrap())
            .unwrap();
        assert_eq!(restored_state, state_bytes);
    }

    #[test]
    fn kernel_requires_secret_resolver_for_secretful_manifest() {
        let store = Arc::new(MemStore::new());
        let mut manifest = empty_manifest();
        manifest.secrets.push(SecretEntry::Decl(SecretDecl {
            alias: "payments/stripe".into(),
            version: 1,
            binding_id: "stripe:prod".into(),
            expected_digest: None,
            policy: None,
        }));
        let loaded = LoadedManifest {
            manifest,
            secrets: vec![SecretDecl {
                alias: "payments/stripe".into(),
                version: 1,
                binding_id: "stripe:prod".into(),
                expected_digest: None,
                policy: None,
            }],
            modules: HashMap::new(),
            plans: HashMap::new(),
            effects: HashMap::new(),
            caps: HashMap::new(),
            policies: HashMap::new(),
            schemas: HashMap::new(),
            effect_catalog: EffectCatalog::new(),
        };

        let result = Kernel::from_loaded_manifest(store, loaded, Box::new(MemJournal::new()));

        assert!(matches!(result, Err(KernelError::SecretResolverMissing)));
    }

    #[test]
    fn tail_scan_returns_entries_after_height() {
        let tmp = TempDir::new().unwrap();
        let manifest_path = tmp.path().join("manifest.cbor");
        write_manifest(&manifest_path, &empty_manifest());

        let store = Arc::new(MemStore::new());
        let mut kernel = KernelBuilder::new(store)
            .from_manifest_path(&manifest_path)
            .expect("kernel");

        let intent = EffectIntentRecord {
            intent_hash: [1u8; 32],
            kind: "http.request".into(),
            cap_name: "cap/http@1".into(),
            params_cbor: vec![1],
            idempotency_key: [2u8; 32],
            origin: IntentOriginRecord::Reducer {
                name: "example/Reducer@1".into(),
            },
        };
        let intent_bytes = serde_cbor::to_vec(&intent).unwrap();
        kernel
            .journal
            .append(JournalEntry::new(JournalKind::EffectIntent, &intent_bytes))
            .unwrap();

        let receipt = EffectReceiptRecord {
            intent_hash: [1u8; 32],
            adapter_id: "stub.http".into(),
            status: aos_effects::ReceiptStatus::Ok,
            payload_cbor: vec![],
            cost_cents: None,
            signature: vec![],
            now_ns: 0,
            logical_now_ns: 0,
            journal_height: 0,
            entropy: Vec::new(),
            manifest_hash: String::new(),
        };
        let receipt_bytes = serde_cbor::to_vec(&receipt).unwrap();
        kernel
            .journal
            .append(JournalEntry::new(
                JournalKind::EffectReceipt,
                &receipt_bytes,
            ))
            .unwrap();

        let scan = kernel.tail_scan_after(0).expect("tail scan");
        assert!(scan.entries.len() >= 2);
        assert_eq!(scan.intents.len(), 1);
        assert_eq!(scan.receipts.len(), 1);
        assert!(
            scan.entries
                .iter()
                .any(|entry| entry.kind == JournalKind::EffectIntent)
        );
        assert!(
            scan.entries
                .iter()
                .any(|entry| entry.kind == JournalKind::EffectReceipt)
        );
        // New worlds start with baseline+manifest journal records.
        assert_eq!(scan.intents[0].seq, 2);
        assert_eq!(scan.receipts[0].seq, 3);
        assert_eq!(scan.intents[0].record.intent_hash, [1u8; 32]);
        assert_eq!(scan.receipts[0].record.intent_hash, [1u8; 32]);
    }
}

fn format_intent_hash(hash: &[u8; 32]) -> String {
    DigestHash::from_bytes(hash)
        .map(|h| h.to_hex())
        .unwrap_or_else(|_| format!("{:?}", hash))
}
