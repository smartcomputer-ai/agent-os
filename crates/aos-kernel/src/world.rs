use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use aos_air_exec::{Value as ExprValue, ValueKey};
use aos_air_types::{
    AirNode, DefModule, Manifest, Name, NamedRef, TypeExpr, builtins,
    plan_literals::{SchemaIndex, normalize_plan_literals},
};
use aos_cbor::{Hash, Hash as DigestHash, to_canonical_cbor};
use aos_effects::{EffectIntent, EffectKind, EffectReceipt};
use aos_store::Store;
use aos_wasm_abi::{ABI_VERSION, CallContext, DomainEvent, ReducerInput, ReducerOutput};
use serde_cbor;

use crate::capability::CapabilityResolver;
use crate::effects::EffectManager;
use crate::error::KernelError;
use crate::event::{KernelEvent, ReducerEvent};
use crate::governance::{GovernanceManager, ManifestPatch, ProposalState};
use crate::journal::fs::FsJournal;
use crate::journal::mem::MemJournal;
use crate::journal::{
    AppliedRecord, ApprovedRecord, ApprovalDecisionRecord, DomainEventRecord,
    EffectIntentRecord, EffectReceiptRecord, GovernanceRecord, IntentOriginRecord, Journal,
    JournalEntry, JournalKind, JournalRecord, JournalSeq, OwnedJournalEntry, PlanResultRecord,
    ProposedRecord, ShadowReportRecord, SnapshotRecord,
};
use crate::manifest::{LoadedManifest, ManifestLoader};
use crate::plan::{PlanInstance, PlanRegistry, ReducerSchema};
use crate::policy::{AllowAllPolicy, RulePolicy};
use crate::receipts::{ReducerEffectContext, build_reducer_receipt_event};
use crate::reducer::ReducerRegistry;
use crate::scheduler::{Scheduler, Task};
use crate::secret::{PlaceholderSecretResolver, SharedSecretResolver};
use crate::shadow::{
    DeltaKind, LedgerDelta, LedgerKind, ShadowConfig, ShadowExecutor, ShadowHarness, ShadowSummary,
};
use crate::snapshot::{
    EffectIntentSnapshot, KernelSnapshot, PendingPlanReceiptSnapshot, PlanResultSnapshot,
    ReducerReceiptSnapshot, receipts_to_vecdeque,
};

const RECENT_RECEIPT_CACHE: usize = 512;
const RECENT_PLAN_RESULT_CACHE: usize = 256;

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
    module_defs: HashMap<Name, aos_air_types::DefModule>,
    reducers: ReducerRegistry<S>,
    router: HashMap<String, Vec<Name>>,
    plan_registry: PlanRegistry,
    schema_index: Arc<SchemaIndex>,
    reducer_schemas: Arc<HashMap<Name, ReducerSchema>>,
    plan_instances: HashMap<u64, PlanInstance>,
    plan_triggers: HashMap<String, Vec<PlanTriggerBinding>>,
    waiting_events: HashMap<String, Vec<u64>>,
    pending_receipts: HashMap<[u8; 32], u64>,
    pending_reducer_receipts: HashMap<[u8; 32], ReducerEffectContext>,
    recent_receipts: VecDeque<[u8; 32]>,
    recent_receipt_index: HashSet<[u8; 32]>,
    plan_results: VecDeque<PlanResultEntry>,
    scheduler: Scheduler,
    effect_manager: EffectManager,
    reducer_state: HashMap<Name, Vec<u8>>,
    journal: Box<dyn Journal>,
    suppress_journal: bool,
    governance: GovernanceManager,
    secret_resolver: Option<SharedSecretResolver>,
    allow_placeholder_secrets: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanResultEntry {
    pub plan_name: String,
    pub plan_id: u64,
    pub output_schema: String,
    pub value_cbor: Vec<u8>,
}

#[derive(Clone, Debug)]
struct PlanTriggerBinding {
    plan: String,
    correlate_by: Option<String>,
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
}

impl<S: Store + 'static> Kernel<S> {
    pub fn from_loaded_manifest(
        store: Arc<S>,
        loaded: LoadedManifest,
        journal: Box<dyn Journal>,
    ) -> Result<Self, KernelError> {
        Self::from_loaded_manifest_with_config(store, loaded, journal, KernelConfig::default())
    }

    pub fn from_loaded_manifest_with_config(
        store: Arc<S>,
        loaded: LoadedManifest,
        journal: Box<dyn Journal>,
        config: KernelConfig,
    ) -> Result<Self, KernelError> {
        let secret_resolver = select_secret_resolver(&loaded.manifest, &config)?;
        let mut router = HashMap::new();
        if let Some(routing) = loaded.manifest.routing.as_ref() {
            for route in &routing.events {
                router
                    .entry(route.event.as_str().to_string())
                    .or_insert_with(Vec::new)
                    .push(route.reducer.clone());
            }
        }
        let mut plan_registry = PlanRegistry::default();
        for plan in loaded.plans.values() {
            plan_registry.register(plan.clone());
        }
        let mut plan_triggers = HashMap::new();
        for trigger in &loaded.manifest.triggers {
            plan_triggers
                .entry(trigger.event.as_str().to_string())
                .or_insert_with(Vec::new)
                .push(PlanTriggerBinding {
                    plan: trigger.plan.clone(),
                    correlate_by: trigger.correlate_by.clone(),
                });
        }
        for bindings in plan_triggers.values_mut() {
            bindings.sort_by(|a, b| a.plan.cmp(&b.plan));
        }
        let schema_index = Arc::new(build_schema_index_from_loaded(store.as_ref(), &loaded)?);
        let capability_resolver = CapabilityResolver::from_manifest(
            &loaded.manifest,
            &loaded.caps,
            schema_index.as_ref(),
        )?;
        ensure_plan_capabilities(&loaded.plans, &capability_resolver)?;
        ensure_module_capabilities(&loaded.manifest, &capability_resolver)?;
        let reducer_schemas = Arc::new(build_reducer_schemas(
            &loaded.modules,
            schema_index.as_ref(),
        )?);
        let policy_gate: Box<dyn crate::policy::PolicyGate> = match loaded
            .manifest
            .defaults
            .as_ref()
            .and_then(|defaults| defaults.policy.clone())
        {
            Some(policy_name) => {
                let def = loaded.policies.get(&policy_name).ok_or_else(|| {
                    KernelError::Manifest(format!(
                        "policy '{policy_name}' referenced by manifest defaults was not found"
                    ))
                })?;
                Box::new(RulePolicy::from_def(def))
            }
            None => Box::new(AllowAllPolicy),
        };

        let mut kernel = Self {
            store: store.clone(),
            manifest: loaded.manifest.clone(),
            module_defs: loaded.modules,
            schema_index: schema_index.clone(),
            reducer_schemas: reducer_schemas.clone(),
            reducers: ReducerRegistry::new(store, config.module_cache_dir.clone())?,
            router,
            plan_registry,
            plan_instances: HashMap::new(),
            plan_triggers,
            waiting_events: HashMap::new(),
            pending_receipts: HashMap::new(),
            pending_reducer_receipts: HashMap::new(),
            recent_receipts: VecDeque::new(),
            recent_receipt_index: HashSet::new(),
            plan_results: VecDeque::new(),
            scheduler: Scheduler::default(),
            effect_manager: EffectManager::new(
                capability_resolver,
                policy_gate,
                if loaded.manifest.secrets.is_empty() {
                    None
                } else {
                    Some(crate::secret::SecretCatalog::new(&loaded.manifest.secrets))
                },
                secret_resolver.clone(),
            ),
            reducer_state: HashMap::new(),
            journal,
            suppress_journal: false,
            governance: GovernanceManager::new(),
            secret_resolver: secret_resolver.clone(),
            allow_placeholder_secrets: config.allow_placeholder_secrets,
        };
        if config.eager_module_load {
            for (name, module_def) in kernel.module_defs.iter() {
                kernel.reducers.ensure_loaded(name, module_def)?;
            }
        }
        kernel.replay_existing_entries()?;
        Ok(kernel)
    }

    pub fn enqueue_event(&mut self, event: KernelEvent) {
        match event {
            KernelEvent::Reducer(ev) => self.scheduler.push_reducer(ev),
        }
    }

    pub fn submit_domain_event(&mut self, schema: impl Into<String>, value: Vec<u8>) {
        let event = DomainEvent::new(schema.into(), value);
        let _ = self.record_domain_event(&event);
        self.scheduler.push_reducer(ReducerEvent { event });
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

    fn handle_reducer_event(&mut self, event: ReducerEvent) -> Result<(), KernelError> {
        let reducers = self
            .router
            .get(&event.event.schema)
            .cloned()
            .unwrap_or_default();
        for reducer_name in reducers {
            let module_def = self
                .module_defs
                .get(&reducer_name)
                .ok_or_else(|| KernelError::ReducerNotFound(reducer_name.clone()))?;
            self.reducers.ensure_loaded(&reducer_name, module_def)?;

            let input = ReducerInput {
                version: ABI_VERSION,
                state: self.reducer_state.get(&reducer_name).cloned(),
                event: event.event.clone(),
                ctx: CallContext::new(false, None),
            };
            let output = self.reducers.invoke(&reducer_name, &input)?;
            self.handle_reducer_output(reducer_name.clone(), output)?;
        }
        self.start_plans_for_event(&event.event)?;
        Ok(())
    }

    fn handle_reducer_output(
        &mut self,
        reducer_name: String,
        output: ReducerOutput,
    ) -> Result<(), KernelError> {
        match output.state {
            Some(state) => {
                self.reducer_state.insert(reducer_name.clone(), state);
            }
            None => {
                self.reducer_state.remove(&reducer_name);
            }
        }
        for event in output.domain_events {
            self.record_domain_event(&event)?;
            self.deliver_event_to_waiting_plans(&event)?;
            self.scheduler.push_reducer(ReducerEvent { event });
        }
        for effect in &output.effects {
            let slot = effect.cap_slot.clone().unwrap_or_else(|| "default".into());
            let cap_name = self
                .manifest
                .module_bindings
                .get(&reducer_name)
                .and_then(|binding| binding.slots.get(&slot))
                .ok_or_else(|| KernelError::CapabilityBindingMissing {
                    reducer: reducer_name.clone(),
                    slot: slot.clone(),
                })?
                .clone();
            let intent =
                self.effect_manager
                    .enqueue_reducer_effect(&reducer_name, &cap_name, effect)?;
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
                ),
            );
        }
        Ok(())
    }

    pub fn drain_effects(&mut self) -> Vec<aos_effects::EffectIntent> {
        self.effect_manager.drain()
    }

    pub fn create_snapshot(&mut self) -> Result<(), KernelError> {
        self.tick_until_idle()?;
        if !self.scheduler.is_empty() {
            return Err(KernelError::SnapshotUnavailable(
                "scheduler must be idle before snapshot".into(),
            ));
        }
        let height = self.journal.next_seq();
        let reducer_state = self
            .reducer_state
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let recent_receipts: Vec<[u8; 32]> = self.recent_receipts.iter().cloned().collect();
        let plan_instances = self
            .plan_instances
            .values()
            .map(|instance| instance.snapshot())
            .collect();
        let pending_plan_receipts = self
            .pending_receipts
            .iter()
            .map(|(hash, plan_id)| PendingPlanReceiptSnapshot {
                plan_id: *plan_id,
                intent_hash: *hash,
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
        let snapshot = KernelSnapshot::new(
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
        );
        let bytes = serde_cbor::to_vec(&snapshot)
            .map_err(|err| KernelError::SnapshotDecode(err.to_string()))?;
        let hash = self.store.put_blob(&bytes)?;
        self.append_record(JournalRecord::Snapshot(SnapshotRecord {
            snapshot_ref: hash.to_hex(),
            height,
        }))?;
        Ok(())
    }

    fn replay_existing_entries(&mut self) -> Result<(), KernelError> {
        let entries = self.journal.load_from(0)?;
        if entries.is_empty() {
            return Ok(());
        }
        let mut resume_seq: Option<JournalSeq> = None;
        let mut latest_snapshot: Option<SnapshotRecord> = None;
        for entry in &entries {
            if matches!(entry.kind, JournalKind::Snapshot) {
                let record: JournalRecord = serde_cbor::from_slice(&entry.payload)
                    .map_err(|err| KernelError::Journal(err.to_string()))?;
                if let JournalRecord::Snapshot(snapshot) = record {
                    latest_snapshot = Some(snapshot);
                }
            }
        }
        if let Some(snapshot) = latest_snapshot {
            resume_seq = Some(snapshot.height);
            self.load_snapshot(&snapshot)?;
        }
        self.suppress_journal = true;
        for entry in entries {
            if resume_seq.map_or(false, |seq| entry.seq <= seq) {
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
                self.submit_domain_event(event.schema, event.value);
                self.tick_until_idle()?;
            }
            JournalRecord::EffectIntent(record) => {
                self.restore_effect_intent(record)?;
            }
            JournalRecord::EffectReceipt(record) => {
                let receipt = EffectReceipt {
                    intent_hash: record.intent_hash,
                    adapter_id: record.adapter_id,
                    status: record.status,
                    payload_cbor: record.payload_cbor,
                    cost_cents: record.cost_cents,
                    signature: record.signature,
                };
                self.handle_receipt(receipt)?;
                self.tick_until_idle()?;
            }
            JournalRecord::Governance(record) => {
                self.governance.apply_record(&record);
            }
            JournalRecord::PlanResult(record) => {
                self.restore_plan_result(record);
            }
            _ => {}
        }
        Ok(())
    }

    fn load_snapshot(&mut self, record: &SnapshotRecord) -> Result<(), KernelError> {
        let hash = Hash::from_hex_str(&record.snapshot_ref)
            .map_err(|err| KernelError::SnapshotDecode(err.to_string()))?;
        let bytes = self.store.get_blob(hash)?;
        let snapshot: KernelSnapshot = serde_cbor::from_slice(&bytes)
            .map_err(|err| KernelError::SnapshotDecode(err.to_string()))?;
        self.apply_snapshot(snapshot)
    }

    fn apply_snapshot(&mut self, snapshot: KernelSnapshot) -> Result<(), KernelError> {
        self.reducer_state = snapshot.reducer_state_entries().iter().cloned().collect();
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
            let instance = PlanInstance::from_snapshot(
                inst_snapshot,
                plan,
                self.schema_index.clone(),
                self.reducer_schemas.clone(),
            );
            self.plan_instances.insert(instance.id, instance);
        }

        self.pending_receipts = snapshot
            .pending_plan_receipts()
            .iter()
            .cloned()
            .map(|snap| (snap.intent_hash, snap.plan_id))
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
                    .or_insert_with(|| ReducerEffectContext::new(name, effect_kind, params_cbor));
            }
            IntentOriginRecord::Plan { name: _, plan_id } => {
                self.reconcile_plan_replay_identity(plan_id, record.intent_hash);
                self.pending_receipts.insert(record.intent_hash, plan_id);
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

    pub fn reducer_state(&self, reducer: &str) -> Option<&Vec<u8>> {
        self.reducer_state.get(reducer)
    }

    pub fn pending_plan_receipts(&self) -> Vec<(u64, [u8; 32])> {
        self.pending_receipts
            .iter()
            .map(|(hash, plan_id)| (*plan_id, *hash))
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
                })
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
        let summary_bytes = serde_cbor::to_vec(&summary)
            .map_err(|err| KernelError::Manifest(format!("encode summary: {err}")))?;
        let record = GovernanceRecord::ShadowReport(ShadowReportRecord {
            proposal_id,
            patch_hash: proposal.patch_hash.clone(),
            summary_cbor: Some(summary_bytes),
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

    fn load_manifest_patch(&self, hash_hex: &str) -> Result<ManifestPatch, KernelError> {
        let hash = Hash::from_hex_str(hash_hex)
            .map_err(|err| KernelError::Manifest(format!("invalid patch hash: {err}")))?;
        let bytes = self.store.get_blob(hash)?;
        let patch: ManifestPatch = serde_cbor::from_slice(&bytes)
            .map_err(|err| KernelError::Manifest(format!("decode patch: {err}")))?;
        Ok(patch)
    }

    fn swap_manifest(&mut self, patch: &ManifestPatch) -> Result<(), KernelError> {
        let loaded = patch.to_loaded_manifest();
        let schema_index = Arc::new(build_schema_index_from_loaded(
            self.store.as_ref(),
            &loaded,
        )?);
        let reducer_schemas = Arc::new(build_reducer_schemas(&loaded.modules, &schema_index)?);
        let capability_resolver = CapabilityResolver::from_manifest(
            &loaded.manifest,
            &loaded.caps,
            schema_index.as_ref(),
        )?;
        let policy_gate: Box<dyn crate::policy::PolicyGate> = match loaded
            .manifest
            .defaults
            .as_ref()
            .and_then(|defaults| defaults.policy.clone())
        {
            Some(policy_name) => {
                let def = loaded.policies.get(&policy_name).ok_or_else(|| {
                    KernelError::Manifest(format!(
                        "policy '{policy_name}' referenced by manifest defaults was not found"
                    ))
                })?;
                Box::new(RulePolicy::from_def(def))
            }
            None => Box::new(AllowAllPolicy),
        };

        self.manifest = loaded.manifest;
        self.module_defs = loaded.modules;
        self.plan_registry = PlanRegistry::default();
        for plan in loaded.plans.values() {
            self.plan_registry.register(plan.clone());
        }

        self.router.clear();
        if let Some(routing) = self.manifest.routing.as_ref() {
            for route in &routing.events {
                self.router
                    .entry(route.event.as_str().to_string())
                    .or_insert_with(Vec::new)
                    .push(route.reducer.clone());
            }
        }

        let mut plan_triggers = HashMap::new();
        for trigger in &self.manifest.triggers {
            plan_triggers
                .entry(trigger.event.as_str().to_string())
                .or_insert_with(Vec::new)
                .push(PlanTriggerBinding {
                    plan: trigger.plan.clone(),
                    correlate_by: trigger.correlate_by.clone(),
                });
        }
        for bindings in plan_triggers.values_mut() {
            bindings.sort_by(|a, b| a.plan.cmp(&b.plan));
        }
        self.plan_triggers = plan_triggers;

        self.plan_instances.clear();
        self.waiting_events.clear();
        self.pending_receipts.clear();
        self.pending_reducer_receipts.clear();
        self.recent_receipts.clear();
        self.recent_receipt_index.clear();
        self.plan_results.clear();

        self.schema_index = schema_index;
        self.reducer_schemas = reducer_schemas;
        self.secret_resolver = ensure_secret_resolver(
            &self.manifest,
            self.secret_resolver.clone(),
            self.allow_placeholder_secrets,
        )?;
        let secret_catalog = if self.manifest.secrets.is_empty() {
            None
        } else {
            Some(crate::secret::SecretCatalog::new(&self.manifest.secrets))
        };
        self.effect_manager = EffectManager::new(
            capability_resolver,
            policy_gate,
            secret_catalog,
            self.secret_resolver.clone(),
        );
        Ok(())
    }

    fn compute_ledger_deltas(current: &Manifest, candidate: &Manifest) -> Vec<LedgerDelta> {
        let mut deltas = Vec::new();
        deltas.extend(diff_named_refs(
            &current.caps,
            &candidate.caps,
            LedgerKind::Capability,
        ));
        deltas.extend(diff_named_refs(
            &current.policies,
            &candidate.policies,
            LedgerKind::Policy,
        ));

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

    fn start_plans_for_event(&mut self, event: &DomainEvent) -> Result<(), KernelError> {
        if let Some(plan_bindings) = self.plan_triggers.get(&event.schema) {
            for binding in plan_bindings {
                if let Some(plan_def) = self.plan_registry.get(&binding.plan) {
                    let input: ExprValue = serde_cbor::from_slice(&event.value).map_err(|err| {
                        KernelError::Manifest(format!(
                            "failed to decode plan input for {}: {err}",
                            binding.plan
                        ))
                    })?;
                    let correlation =
                        determine_correlation_value(binding, &input, event.key.as_ref());
                    let instance_id = self.scheduler.alloc_plan_id();
                    let instance = PlanInstance::new(
                        instance_id,
                        plan_def.clone(),
                        input,
                        self.schema_index.clone(),
                        self.reducer_schemas.clone(),
                        correlation,
                    );
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
                let outcome = instance.tick(&mut self.effect_manager)?;
                (name, outcome, snapshot.step_states)
            };
            for event in &outcome.raised_events {
                self.record_domain_event(event)?;
                self.deliver_event_to_waiting_plans(event)?;
                self.scheduler.push_reducer(ReducerEvent {
                    event: event.clone(),
                });
            }
            for intent in &outcome.intents_enqueued {
                self.record_effect_intent(
                    intent,
                    IntentOriginRecord::Plan {
                        name: plan_name.clone(),
                        plan_id: id,
                    },
                )?;
            }
            for hash in &outcome.waiting_receipts {
                self.pending_receipts.insert(*hash, id);
            }
            if let Some(schema) = outcome.waiting_event.clone() {
                self.waiting_events.entry(schema).or_default().push(id);
            }
            if outcome.completed {
                if !self.suppress_journal {
                    if let (Some(value_cbor), Some(output_schema)) =
                        (outcome.result_cbor.clone(), outcome.result_schema.clone())
                    {
                        let entry =
                            PlanResultEntry::new(plan_name.clone(), id, output_schema, value_cbor);
                        self.record_plan_result(&entry)?;
                        self.push_plan_result_entry(entry);
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
        if let Some(plan_id) = self.pending_receipts.remove(&receipt.intent_hash) {
            self.record_effect_receipt(&receipt)?;
            if let Some(instance) = self.plan_instances.get_mut(&plan_id) {
                if instance.deliver_receipt(receipt.intent_hash, &receipt.payload_cbor)? {
                    self.scheduler.push_plan(plan_id);
                }
                self.remember_receipt(receipt.intent_hash);
                return Ok(());
            } else {
                log::warn!(
                    "receipt {} arrived for completed plan {}",
                    format_intent_hash(&receipt.intent_hash),
                    plan_id
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
            self.record_effect_receipt(&receipt)?;
            let event = build_reducer_receipt_event(&context, &receipt)?;
            self.record_domain_event(&event)?;
            self.scheduler.push_reducer(ReducerEvent { event });
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

    fn deliver_event_to_waiting_plans(&mut self, event: &DomainEvent) -> Result<(), KernelError> {
        if let Some(mut plan_ids) = self.waiting_events.remove(&event.schema) {
            let mut still_waiting = Vec::new();
            for id in plan_ids.drain(..) {
                if let Some(instance) = self.plan_instances.get_mut(&id) {
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

    fn record_domain_event(&mut self, event: &DomainEvent) -> Result<(), KernelError> {
        let record = JournalRecord::DomainEvent(DomainEventRecord {
            schema: event.schema.clone(),
            value: event.value.clone(),
            key: event.key.clone(),
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

    fn record_effect_receipt(&mut self, receipt: &EffectReceipt) -> Result<(), KernelError> {
        let record = JournalRecord::EffectReceipt(EffectReceiptRecord {
            intent_hash: receipt.intent_hash,
            adapter_id: receipt.adapter_id.clone(),
            status: receipt.status.clone(),
            payload_cbor: receipt.payload_cbor.clone(),
            cost_cents: receipt.cost_cents,
            signature: receipt.signature.clone(),
        });
        self.append_record(record)
    }
}

fn select_secret_resolver(
    manifest: &Manifest,
    config: &KernelConfig,
) -> Result<Option<SharedSecretResolver>, KernelError> {
    ensure_secret_resolver(
        manifest,
        config.secret_resolver.clone(),
        config.allow_placeholder_secrets,
    )
}

fn ensure_secret_resolver(
    manifest: &Manifest,
    provided: Option<SharedSecretResolver>,
    allow_placeholder: bool,
) -> Result<Option<SharedSecretResolver>, KernelError> {
    if manifest.secrets.is_empty() {
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

fn canonicalize_patch<S: Store>(
    store: &S,
    patch: ManifestPatch,
) -> Result<ManifestPatch, KernelError> {
    let mut canonical = patch.clone();

    let mut schema_map = HashMap::new();
    for builtin in builtins::builtin_schemas() {
        schema_map.insert(builtin.schema.name.clone(), builtin.schema.ty.clone());
    }
    let mut module_map: HashMap<Name, DefModule> = HashMap::new();

    for node in &canonical.nodes {
        match node {
            AirNode::Defschema(schema) => {
                schema_map.insert(schema.name.clone(), schema.ty.clone());
            }
            AirNode::Defmodule(module) => {
                module_map.insert(module.name.clone(), module.clone());
            }
            _ => {}
        }
    }

    extend_schema_map_from_store(store, &canonical.manifest.schemas, &mut schema_map)?;
    extend_module_map_from_store(store, &canonical.manifest.modules, &mut module_map)?;

    let schema_index = SchemaIndex::new(schema_map);
    for node in canonical.nodes.iter_mut() {
        if let AirNode::Defplan(plan) = node {
            normalize_plan_literals(plan, &schema_index, &module_map).map_err(|err| {
                KernelError::Manifest(format!(
                    "plan '{}' literal normalization failed: {err}",
                    plan.name
                ))
            })?;
        }
    }

    Ok(canonical)
}

fn extend_schema_map_from_store<S: Store>(
    store: &S,
    refs: &[NamedRef],
    schemas: &mut HashMap<String, TypeExpr>,
) -> Result<(), KernelError> {
    for reference in refs {
        if schemas.contains_key(reference.name.as_str()) {
            continue;
        }
        if let Some(hash) = parse_nonzero_hash(reference.hash.as_str())? {
            let node: AirNode = store.get_node(hash)?;
            if let AirNode::Defschema(schema) = node {
                schemas.insert(schema.name.clone(), schema.ty.clone());
            }
        }
    }
    Ok(())
}

fn extend_module_map_from_store<S: Store>(
    store: &S,
    refs: &[NamedRef],
    modules: &mut HashMap<Name, DefModule>,
) -> Result<(), KernelError> {
    for reference in refs {
        if modules.contains_key(reference.name.as_str()) {
            continue;
        }
        if let Some(hash) = parse_nonzero_hash(reference.hash.as_str())? {
            let node: AirNode = store.get_node(hash)?;
            if let AirNode::Defmodule(module) = node {
                modules.insert(module.name.clone(), module);
            }
        }
    }
    Ok(())
}

fn parse_nonzero_hash(value: &str) -> Result<Option<Hash>, KernelError> {
    let hash = Hash::from_hex_str(value)
        .map_err(|err| KernelError::Manifest(format!("invalid hash '{value}': {err}")))?;
    if hash.as_bytes().iter().all(|b| *b == 0) {
        Ok(None)
    } else {
        Ok(Some(hash))
    }
}

fn diff_named_refs(
    current: &[NamedRef],
    candidate: &[NamedRef],
    ledger: LedgerKind,
) -> Vec<LedgerDelta> {
    let mut deltas = Vec::new();
    let current_map: HashMap<&str, &NamedRef> = current
        .iter()
        .map(|reference| (reference.name.as_str(), reference))
        .collect();
    let next_map: HashMap<&str, &NamedRef> = candidate
        .iter()
        .map(|reference| (reference.name.as_str(), reference))
        .collect();

    for (name, reference) in next_map.iter() {
        match current_map.get(name) {
            None => deltas.push(LedgerDelta {
                ledger,
                name: reference.name.as_str().to_string(),
                change: DeltaKind::Added,
            }),
            Some(current_ref) if current_ref.hash.as_str() != reference.hash.as_str() => deltas
                .push(LedgerDelta {
                    ledger,
                    name: reference.name.as_str().to_string(),
                    change: DeltaKind::Changed,
                }),
            _ => {}
        }
    }

    for (name, reference) in current_map.iter() {
        if !next_map.contains_key(name) {
            deltas.push(LedgerDelta {
                ledger,
                name: reference.name.as_str().to_string(),
                change: DeltaKind::Removed,
            })
        }
    }

    deltas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::mem::MemJournal;
    use aos_air_types::{HashRef, SecretDecl};
    use aos_store::MemStore;

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

    fn empty_manifest() -> Manifest {
        Manifest {
            schemas: vec![],
            modules: vec![],
            plans: vec![],
            caps: vec![],
            policies: vec![],
            secrets: vec![],
            defaults: None,
            module_bindings: Default::default(),
            routing: None,
            triggers: vec![],
        }
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

    #[test]
    fn kernel_requires_secret_resolver_for_secretful_manifest() {
        let store = Arc::new(MemStore::new());
        let mut manifest = empty_manifest();
        manifest.secrets.push(SecretDecl {
            alias: "payments/stripe".into(),
            version: 1,
            binding_id: "stripe:prod".into(),
            expected_digest: None,
            policy: None,
        });
        let loaded = LoadedManifest {
            manifest,
            modules: HashMap::new(),
            plans: HashMap::new(),
            caps: HashMap::new(),
            policies: HashMap::new(),
            schemas: HashMap::new(),
        };

        let result = Kernel::from_loaded_manifest(store, loaded, Box::new(MemJournal::new()));

        assert!(matches!(result, Err(KernelError::SecretResolverMissing)));
    }
}

fn format_intent_hash(hash: &[u8; 32]) -> String {
    DigestHash::from_bytes(hash)
        .map(|h| h.to_hex())
        .unwrap_or_else(|_| format!("{:?}", hash))
}

fn build_schema_index_from_loaded<S: Store>(
    store: &S,
    loaded: &LoadedManifest,
) -> Result<SchemaIndex, KernelError> {
    let mut schema_map = HashMap::new();
    for builtin in builtins::builtin_schemas() {
        schema_map.insert(builtin.schema.name.clone(), builtin.schema.ty.clone());
    }
    for (name, schema) in &loaded.schemas {
        schema_map.insert(name.clone(), schema.ty.clone());
    }
    extend_schema_map_from_store(store, &loaded.manifest.schemas, &mut schema_map)?;
    Ok(SchemaIndex::new(schema_map))
}

fn build_reducer_schemas(
    modules: &HashMap<Name, aos_air_types::DefModule>,
    schema_index: &SchemaIndex,
) -> Result<HashMap<Name, ReducerSchema>, KernelError> {
    let mut map = HashMap::new();
    for (name, module) in modules {
        if let Some(reducer) = module.abi.reducer.as_ref() {
            let schema_name = reducer.event.as_str();
            let event_schema = schema_index
                .get(schema_name)
                .ok_or_else(|| {
                    KernelError::Manifest(format!(
                        "schema '{schema_name}' not found for reducer '{name}'"
                    ))
                })?
                .clone();
            let key_schema = if let Some(key_ref) = &module.key_schema {
                let schema_name = key_ref.as_str();
                Some(
                    schema_index
                        .get(schema_name)
                        .ok_or_else(|| {
                            KernelError::Manifest(format!(
                                "schema '{schema_name}' not found for reducer '{name}' key"
                            ))
                        })?
                        .clone(),
                )
            } else {
                None
            };
            map.insert(
                name.clone(),
                ReducerSchema {
                    event_schema_name: schema_name.to_string(),
                    event_schema,
                    key_schema,
                },
            );
        }
    }
    Ok(map)
}

fn ensure_plan_capabilities(
    plans: &HashMap<Name, aos_air_types::DefPlan>,
    resolver: &CapabilityResolver,
) -> Result<(), KernelError> {
    for plan in plans.values() {
        for cap in &plan.required_caps {
            if !resolver.has_grant(cap) {
                return Err(KernelError::PlanCapabilityMissing {
                    plan: plan.name.clone(),
                    cap: cap.clone(),
                });
            }
        }
    }
    Ok(())
}

fn ensure_module_capabilities(
    manifest: &Manifest,
    resolver: &CapabilityResolver,
) -> Result<(), KernelError> {
    for (module, binding) in &manifest.module_bindings {
        for (_slot, cap) in &binding.slots {
            if !resolver.has_grant(cap) {
                return Err(KernelError::ModuleCapabilityMissing {
                    module: module.clone(),
                    cap: cap.clone(),
                });
            }
        }
    }
    Ok(())
}
