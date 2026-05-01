use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use aos_authoring::{
    WorkflowBuildProfile, build_loaded_manifest_from_air_sources,
    build_loaded_manifest_from_authored_paths, default_world_module_dir,
    load_required_secret_value_map, load_world_config, local_state_paths,
    materialize_discovered_cargo_modules, reset_local_runtime_state, resolve_world_air_sources,
};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effects::builtins::{
    BlobGetReceipt, BlobPutReceipt, LlmGenerateReceipt, TimerSetParams, TimerSetReceipt,
};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus, effect_ops};
use aos_kernel::cell_index::CellMeta;
use aos_kernel::journal::Journal;
use aos_kernel::{
    Kernel, KernelConfig, LoadedManifest, MapSecretResolver, MemStore, Store,
    workflow_trace_summary_with_routes,
};
use aos_node::control::StateCellSummary;
use aos_node::{CborPayload, DomainEventIngress, NodeWorldHarness as AosNodeWorldHarness, WorldId};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Serialize;
use serde_json::Value as JsonValue;
use uuid::Uuid;

pub type HostError = anyhow::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectMode {
    #[default]
    Scripted,
    Twin,
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectExecutionClass {
    InlineInternal,
    OwnerLocalTimer,
    ExternalAsync,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct DrainStatus {
    pub idle: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CycleOutcome {
    pub effects_dispatched: usize,
    pub receipts_applied: usize,
    pub final_drain: DrainStatus,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct KernelQuiescenceStatus {
    pub kernel_idle: bool,
    pub runtime_quiescent: bool,
    pub workflow_queue_pending: bool,
    pub queued_effects: usize,
    pub pending_workflow_receipts: usize,
    pub inflight_workflow_intents: usize,
    pub non_terminal_workflow_instances: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct QuiescenceStatus {
    pub kernel: KernelQuiescenceStatus,
    pub runtime_quiescent: bool,
    pub timers_pending: usize,
    pub next_timer_deadline_ns: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct HarnessEvidence {
    pub cycles_run: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessArtifacts {
    pub journal_entries: Vec<JsonValue>,
    pub trace_summary: JsonValue,
    pub evidence: HarnessEvidence,
}

#[derive(Debug, Clone)]
pub struct HarnessCell {
    pub key_hash: Vec<u8>,
    pub key_bytes: Vec<u8>,
    pub state_hash: String,
    pub size: u64,
    pub last_active_ns: u64,
}

#[derive(Debug)]
struct PendingTimer {
    intent_hash: [u8; 32],
    params: TimerSetParams,
}

pub struct BootstrappedWorldHarness {
    pub harness: NodeRuntimeWorldHarness,
    pub world_id: Uuid,
    pub warnings: Vec<String>,
}

struct WorkflowHarnessCore<S: Store + Clone + Send + Sync + 'static> {
    store: Arc<S>,
    loaded: LoadedManifest,
    kernel_config: KernelConfig,
    kernel: Kernel<S>,
    effect_mode: EffectMode,
    pending_timers: Vec<PendingTimer>,
    scripted_effects: Vec<EffectIntent>,
    logical_now_ns: u64,
    cycles_run: u64,
}

pub struct NodeRuntimeWorldHarness {
    inner: AosNodeWorldHarness,
    world_id: WorldId,
    cycles_run: u64,
}

pub struct RuntimeWorkflowHarness<S: Store + Clone + Send + Sync + 'static> {
    workflow: String,
    inner: WorkflowHarnessCore<S>,
}

pub fn bootstrap_node_world_harness(
    world_root: &Path,
    reset: bool,
    force_build: bool,
    sync_secrets: bool,
    effect_mode: EffectMode,
) -> Result<BootstrappedWorldHarness> {
    if effect_mode != EffectMode::Scripted {
        bail!("runtime-backed WorldHarness currently supports only effect_mode='scripted'");
    }

    if reset {
        reset_local_runtime_state(world_root).context("reset local runtime state")?;
    }

    let paths = local_state_paths(world_root);
    let air_dir = world_root.join("air");
    let module_dir = default_world_module_dir(world_root);
    let (config_path, config) = load_world_config(world_root, None).context("load world config")?;
    let air_sources = resolve_world_air_sources(
        world_root,
        config_path.as_deref(),
        &config,
        &air_dir,
        &module_dir,
    )
    .context("resolve local AIR sources")?;
    let stage_store = MemStore::new();
    materialize_discovered_cargo_modules(
        &air_sources.packages,
        world_root,
        &paths.cache_root(),
        &stage_store,
        WorkflowBuildProfile::Release,
    )
    .context("materialize imported cargo modules")?;
    let (store, loaded) = build_loaded_manifest_from_air_sources(
        &air_sources,
        world_root,
        force_build,
        WorkflowBuildProfile::Release,
    )
    .context("build local world harness manifest")?;
    let warnings = air_sources.warnings;

    let node_harness =
        AosNodeWorldHarness::open(paths.root()).context("open node harness state root")?;
    if sync_secrets {
        let bindings = required_secret_bindings(&loaded);
        if !bindings.is_empty() {
            let values = load_required_secret_value_map(world_root, None, &bindings)
                .context("load synced node secret values")?;
            for (binding_id, plaintext) in values {
                node_harness
                    .control()
                    .put_node_secret(&binding_id, &plaintext)
                    .with_context(|| format!("sync node secret binding '{binding_id}'"))?;
            }
        }
    }
    let world_id = WorldId::from(Uuid::new_v4());
    node_harness
        .create_world_from_loaded_manifest(&store, &loaded, world_id, 0)
        .context("create harness world from loaded manifest")?;

    Ok(BootstrappedWorldHarness {
        harness: NodeRuntimeWorldHarness {
            inner: node_harness,
            world_id,
            cycles_run: 0,
        },
        world_id: world_id.as_uuid(),
        warnings,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn build_runtime_workflow_harness_from_authored_paths_with_secret_config(
    workflow: String,
    air_dir: &Path,
    workflow_dir: Option<&Path>,
    import_roots: &[PathBuf],
    scratch_root: &Path,
    force_build: bool,
    build_profile: WorkflowBuildProfile,
    effect_mode: EffectMode,
    sync_root: Option<&Path>,
    _sync_map: Option<&Path>,
    secret_bindings: Option<HashMap<String, Vec<u8>>>,
) -> Result<RuntimeWorkflowHarness<MemStore>> {
    if effect_mode != EffectMode::Scripted {
        bail!("WorkflowHarness currently supports only effect_mode='scripted'");
    }

    let (store, loaded) = build_loaded_manifest_from_authored_paths(
        air_dir,
        workflow_dir,
        import_roots,
        scratch_root,
        force_build,
        build_profile,
    )
    .context("build authored workflow harness manifest")?;

    let mut kernel_config = KernelConfig::default();
    if let Some(secret_bindings) = secret_bindings {
        kernel_config.secret_resolver = Some(Arc::new(MapSecretResolver::new(secret_bindings)));
    } else if let Some(sync_root) = sync_root {
        let bindings = required_secret_bindings(&loaded);
        if !bindings.is_empty() {
            let values = load_required_secret_value_map(sync_root, None, &bindings)
                .context("load authored harness secret values")?;
            kernel_config.secret_resolver = Some(Arc::new(MapSecretResolver::new(values)));
        }
    }

    Ok(RuntimeWorkflowHarness {
        workflow,
        inner: WorkflowHarnessCore::new(Arc::new(store), loaded, kernel_config, effect_mode)?,
    })
}

fn required_secret_bindings(loaded: &LoadedManifest) -> BTreeSet<String> {
    loaded
        .secrets
        .iter()
        .map(|secret| secret.binding_id.clone())
        .collect()
}

impl<S: Store + Clone + Send + Sync + 'static> WorkflowHarnessCore<S> {
    fn new(
        store: Arc<S>,
        loaded: LoadedManifest,
        kernel_config: KernelConfig,
        effect_mode: EffectMode,
    ) -> Result<Self> {
        let kernel = Kernel::from_loaded_manifest_with_config(
            store.clone(),
            loaded.clone(),
            Journal::new(),
            kernel_config.clone(),
        )
        .context("boot harness kernel")?;
        Self::from_kernel(store, loaded, kernel_config, effect_mode, kernel)
    }

    fn reopen(&self) -> Result<Self> {
        let journal_entries = self.kernel.dump_journal().context("dump harness journal")?;
        let journal = Journal::from_entries(&journal_entries).context("seed reopened journal")?;
        let kernel = Kernel::from_loaded_manifest_with_config(
            self.store.clone(),
            self.loaded.clone(),
            journal,
            self.kernel_config.clone(),
        )
        .context("reopen harness kernel")?;
        let mut reopened = Self::from_kernel(
            self.store.clone(),
            self.loaded.clone(),
            self.kernel_config.clone(),
            self.effect_mode,
            kernel,
        )?;
        reopened.logical_now_ns = self.logical_now_ns;
        reopened.cycles_run = self.cycles_run;
        reopened.restore_open_work()?;
        Ok(reopened)
    }

    fn from_kernel(
        store: Arc<S>,
        loaded: LoadedManifest,
        kernel_config: KernelConfig,
        effect_mode: EffectMode,
        kernel: Kernel<S>,
    ) -> Result<Self> {
        Ok(Self {
            store,
            loaded,
            kernel_config,
            kernel,
            effect_mode,
            pending_timers: Vec::new(),
            scripted_effects: Vec::new(),
            logical_now_ns: 0,
            cycles_run: 0,
        })
    }

    fn restore_open_work(&mut self) -> Result<()> {
        for pending in self.kernel.pending_workflow_receipts_snapshot() {
            let intent = EffectIntent {
                effect: if pending.effect.is_empty() {
                    pending
                        .executor_entrypoint
                        .clone()
                        .unwrap_or_else(|| pending.effect.clone())
                } else {
                    pending.effect.clone()
                },
                effect_hash: pending.effect_hash.clone(),
                executor_module: pending.executor_module.clone(),
                executor_module_hash: pending.executor_module_hash.clone(),
                executor_entrypoint: pending.executor_entrypoint.clone(),
                params_cbor: pending.params_cbor.clone(),
                idempotency_key: pending.idempotency_key,
                intent_hash: pending.intent_hash,
            };
            match classify_effect_intent(&intent) {
                EffectExecutionClass::InlineInternal => {}
                EffectExecutionClass::OwnerLocalTimer => {
                    let params: TimerSetParams = serde_cbor::from_slice(&intent.params_cbor)
                        .context("decode restored timer intent")?;
                    self.pending_timers.push(PendingTimer {
                        intent_hash: intent.intent_hash,
                        params,
                    });
                }
                EffectExecutionClass::ExternalAsync => {
                    self.scripted_effects.push(intent);
                }
            }
        }
        Ok(())
    }

    fn send_event(&mut self, schema: &str, json_value: JsonValue) -> Result<()> {
        self.kernel
            .submit_domain_event_result(schema.to_string(), to_canonical_cbor(&json_value)?)
            .context("submit harness event")
    }

    fn send_command(&mut self, schema: &str, json_value: JsonValue) -> Result<()> {
        self.send_event(schema, json_value)
    }

    fn run_cycle(&mut self, process_due_timers: bool) -> Result<CycleOutcome> {
        self.kernel
            .tick_until_idle()
            .context("drain workflow queue")?;
        let intents = self.kernel.drain_effects().context("drain effect batch")?;
        let mut effects_dispatched = 0usize;
        let mut receipts_applied = 0usize;

        for intent in intents {
            if let Some(receipt) = self
                .kernel
                .handle_internal_intent(&intent)
                .context("handle internal intent")?
            {
                self.kernel
                    .handle_receipt(receipt)
                    .context("apply internal receipt")?;
                receipts_applied = receipts_applied.saturating_add(1);
                continue;
            }

            match classify_effect_intent(&intent) {
                EffectExecutionClass::InlineInternal => {}
                EffectExecutionClass::OwnerLocalTimer => {
                    let params: TimerSetParams = serde_cbor::from_slice(&intent.params_cbor)
                        .context("decode timer intent")?;
                    self.pending_timers.push(PendingTimer {
                        intent_hash: intent.intent_hash,
                        params,
                    });
                    effects_dispatched = effects_dispatched.saturating_add(1);
                }
                EffectExecutionClass::ExternalAsync => {
                    self.scripted_effects.push(intent);
                    effects_dispatched = effects_dispatched.saturating_add(1);
                }
            }
        }

        if receipts_applied > 0 {
            self.kernel
                .tick_until_idle()
                .context("drain after effect receipts")?;
        }
        if process_due_timers {
            receipts_applied = receipts_applied.saturating_add(self.fire_due_timers()?);
        }

        self.cycles_run = self.cycles_run.saturating_add(1);
        let status = self.kernel.quiescence_status();
        Ok(CycleOutcome {
            effects_dispatched,
            receipts_applied,
            final_drain: DrainStatus {
                idle: status.kernel_idle,
            },
        })
    }

    fn run_until_kernel_idle(&mut self) -> Result<QuiescenceStatus> {
        loop {
            let before = self.quiescence_status();
            if before.kernel.kernel_idle {
                return Ok(before);
            }
            let cycle = self.run_cycle(false)?;
            let after = self.quiescence_status();
            if cycle.effects_dispatched == 0 && cycle.receipts_applied == 0 && after == before {
                return Ok(after);
            }
        }
    }

    fn run_until_runtime_quiescent(&mut self) -> Result<QuiescenceStatus> {
        loop {
            let before = self.quiescence_status();
            if before.runtime_quiescent {
                return Ok(before);
            }
            let cycle = self.run_cycle(true)?;
            let after = self.quiescence_status();
            if cycle.effects_dispatched == 0 && cycle.receipts_applied == 0 && after == before {
                return Ok(after);
            }
        }
    }

    fn quiescence_status(&self) -> QuiescenceStatus {
        let kernel = self.kernel.quiescence_status();
        let next_timer_deadline_ns = self
            .pending_timers
            .iter()
            .map(|timer| timer.params.deliver_at_ns)
            .min();
        let runtime_quiescent = kernel.runtime_quiescent
            && self.pending_timers.is_empty()
            && self.scripted_effects.is_empty();
        QuiescenceStatus {
            kernel: KernelQuiescenceStatus {
                kernel_idle: kernel.kernel_idle,
                runtime_quiescent: kernel.runtime_quiescent,
                workflow_queue_pending: kernel.workflow_queue_pending,
                queued_effects: kernel.queued_effects,
                pending_workflow_receipts: kernel.pending_workflow_receipts,
                inflight_workflow_intents: kernel.inflight_workflow_intents,
                non_terminal_workflow_instances: kernel.non_terminal_workflow_instances,
            },
            runtime_quiescent,
            timers_pending: self.pending_timers.len(),
            next_timer_deadline_ns,
        }
    }

    fn pull_effects(&mut self) -> Result<Vec<EffectIntent>> {
        let _ = self.run_cycle(false)?;
        Ok(std::mem::take(&mut self.scripted_effects))
    }

    fn apply_receipt(&mut self, receipt: EffectReceipt) -> Result<()> {
        self.scripted_effects
            .retain(|intent| intent.intent_hash != receipt.intent_hash);
        self.pending_timers
            .retain(|timer| timer.intent_hash != receipt.intent_hash);
        self.kernel
            .handle_receipt(receipt)
            .context("apply harness receipt")?;
        self.kernel
            .tick_until_idle()
            .context("drain after harness receipt")?;
        Ok(())
    }

    fn snapshot(&mut self) -> Result<()> {
        self.kernel
            .create_snapshot()
            .context("create harness snapshot")
    }

    fn trace_summary(&self) -> Result<JsonValue> {
        Ok(workflow_trace_summary_with_routes(&self.kernel, None)?)
    }

    fn time_get(&self) -> u64 {
        self.logical_now_ns
    }

    fn time_set(&mut self, now_ns: u64) -> u64 {
        self.logical_now_ns = self.kernel.set_logical_time_ns(now_ns);
        self.logical_now_ns
    }

    fn time_advance(&mut self, delta_ns: u64) -> u64 {
        self.time_set(self.logical_now_ns.saturating_add(delta_ns))
    }

    fn time_jump_next_due(&mut self) -> Result<Option<u64>> {
        let Some(next_due) = self
            .pending_timers
            .iter()
            .map(|timer| timer.params.deliver_at_ns)
            .min()
        else {
            return Ok(None);
        };
        self.time_set(next_due);
        let _ = self.fire_due_timers()?;
        Ok(Some(next_due))
    }

    fn export_artifacts(&self) -> Result<HarnessArtifacts> {
        let journal_entries = self
            .kernel
            .dump_journal()
            .context("dump harness journal")?
            .into_iter()
            .map(|entry| serde_json::to_value(entry).context("serialize journal entry"))
            .collect::<Result<Vec<_>>>()?;
        Ok(HarnessArtifacts {
            journal_entries,
            trace_summary: self.trace_summary()?,
            evidence: HarnessEvidence {
                cycles_run: self.cycles_run,
            },
        })
    }

    fn receipt_ok(&self, intent_hash: [u8; 32], payload: &JsonValue) -> Result<EffectReceipt> {
        self.receipt_with_status(intent_hash, ReceiptStatus::Ok, payload)
    }

    fn receipt_error(&self, intent_hash: [u8; 32], payload: &JsonValue) -> Result<EffectReceipt> {
        self.receipt_with_status(intent_hash, ReceiptStatus::Error, payload)
    }

    fn receipt_timeout(&self, intent_hash: [u8; 32], payload: &JsonValue) -> Result<EffectReceipt> {
        self.receipt_with_status(intent_hash, ReceiptStatus::Timeout, payload)
    }

    fn receipt_with_status(
        &self,
        intent_hash: [u8; 32],
        status: ReceiptStatus,
        payload: &JsonValue,
    ) -> Result<EffectReceipt> {
        Ok(EffectReceipt {
            intent_hash,
            status,
            payload_cbor: to_canonical_cbor(payload)?,
            cost_cents: None,
            signature: Vec::new(),
        })
    }

    fn receipt_with_status_typed<T: Serialize>(
        &self,
        intent_hash: [u8; 32],
        status: ReceiptStatus,
        payload: &T,
    ) -> Result<EffectReceipt> {
        Ok(EffectReceipt {
            intent_hash,
            status,
            payload_cbor: to_canonical_cbor(payload)?,
            cost_cents: None,
            signature: Vec::new(),
        })
    }

    fn receipt_ok_typed<T: Serialize>(
        &self,
        intent_hash: [u8; 32],
        payload: &T,
    ) -> Result<EffectReceipt> {
        self.receipt_with_status_typed(intent_hash, ReceiptStatus::Ok, payload)
    }

    fn receipt_timer_set_ok(
        &self,
        intent_hash: [u8; 32],
        delivered_at_ns: u64,
        key: Option<String>,
    ) -> Result<EffectReceipt> {
        self.receipt_ok_typed(
            intent_hash,
            &TimerSetReceipt {
                delivered_at_ns,
                key,
            },
        )
    }

    fn receipt_blob_put_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &BlobPutReceipt,
    ) -> Result<EffectReceipt> {
        self.receipt_ok_typed(intent_hash, payload)
    }

    fn receipt_blob_get_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &BlobGetReceipt,
    ) -> Result<EffectReceipt> {
        self.receipt_ok_typed(intent_hash, payload)
    }

    fn receipt_llm_generate_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &LlmGenerateReceipt,
    ) -> Result<EffectReceipt> {
        self.receipt_ok_typed(intent_hash, payload)
    }

    fn state_json(&self, workflow: &str, key: Option<&[u8]>) -> Result<JsonValue> {
        match self
            .kernel
            .workflow_state_bytes(workflow, key)
            .context("load workflow state bytes")?
        {
            Some(bytes) => decode_state_json(&bytes),
            None => Ok(JsonValue::Null),
        }
    }

    fn state_bytes(&self, workflow: &str, key: Option<&[u8]>) -> Option<Vec<u8>> {
        self.kernel
            .workflow_state_bytes(workflow, key)
            .ok()
            .flatten()
    }

    fn list_cells(&self, workflow: &str) -> Result<Vec<HarnessCell>> {
        self.kernel
            .list_cells(workflow)
            .context("list workflow cells")?
            .iter()
            .map(HarnessCell::from_kernel_cell)
            .collect()
    }

    fn blob_bytes(&self, blob_ref: &str) -> Result<Vec<u8>> {
        self.store
            .get_blob(
                Hash::from_hex_str(blob_ref)
                    .map_err(|err| anyhow!("invalid blob ref '{blob_ref}': {err}"))?,
            )
            .context("load workflow blob")
    }

    fn fire_due_timers(&mut self) -> Result<usize> {
        let mut ready = Vec::new();
        let mut pending = Vec::new();
        for timer in self.pending_timers.drain(..) {
            if timer.params.deliver_at_ns <= self.logical_now_ns {
                ready.push(timer);
            } else {
                pending.push(timer);
            }
        }
        self.pending_timers = pending;

        for timer in &ready {
            let receipt = EffectReceipt {
                intent_hash: timer.intent_hash,
                status: ReceiptStatus::Ok,
                payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
                    delivered_at_ns: timer.params.deliver_at_ns,
                    key: timer.params.key.clone(),
                })?,
                cost_cents: None,
                signature: Vec::new(),
            };
            self.kernel
                .handle_receipt(receipt)
                .context("apply timer receipt")?;
        }
        if !ready.is_empty() {
            self.kernel
                .tick_until_idle()
                .context("drain after timer fire")?;
        }
        Ok(ready.len())
    }
}

impl HarnessCell {
    fn from_kernel_cell(cell: &CellMeta) -> Result<Self> {
        Ok(Self {
            key_hash: cell.key_hash.to_vec(),
            key_bytes: cell.key_bytes.clone(),
            state_hash: Hash::from_bytes(&cell.state_hash)
                .context("cell state hash")?
                .to_hex(),
            size: cell.size,
            last_active_ns: cell.last_active_ns,
        })
    }

    fn from_state_cell(cell: &StateCellSummary) -> Self {
        Self {
            key_hash: cell.key_hash.clone(),
            key_bytes: cell.key_bytes.clone(),
            state_hash: cell.state_hash.clone(),
            size: cell.size,
            last_active_ns: cell.last_active_ns,
        }
    }
}

impl NodeRuntimeWorldHarness {
    pub fn send_event(&mut self, schema: &str, json_value: JsonValue) -> Result<()> {
        self.inner.control().enqueue_event(
            self.world_id,
            DomainEventIngress {
                schema: schema.to_string(),
                value: CborPayload::inline(to_canonical_cbor(&json_value)?),
                key: None,
                correlation_id: None,
            },
        )?;
        Ok(())
    }

    pub fn send_command(&mut self, command: &str, json_value: JsonValue) -> Result<()> {
        self.inner
            .control()
            .submit_command(self.world_id, command, None, None, &json_value)?;
        Ok(())
    }

    pub fn run_cycle_batch(&mut self) -> Result<CycleOutcome> {
        self.inner.control().step_world(self.world_id)?;
        self.cycles_run = self.cycles_run.saturating_add(1);
        let status = self.quiescence_status();
        Ok(CycleOutcome {
            effects_dispatched: 0,
            receipts_applied: 0,
            final_drain: DrainStatus {
                idle: status.kernel.kernel_idle,
            },
        })
    }

    pub fn run_until_kernel_idle(&mut self) -> Result<QuiescenceStatus> {
        loop {
            let before = self.quiescence_status();
            if before.kernel.kernel_idle {
                return Ok(before);
            }
            let _ = self.run_cycle_batch()?;
            let after = self.quiescence_status();
            if after == before {
                return Ok(after);
            }
        }
    }

    pub fn run_until_runtime_quiescent(&mut self) -> Result<QuiescenceStatus> {
        loop {
            let before = self.quiescence_status();
            if before.runtime_quiescent {
                return Ok(before);
            }
            let _ = self.run_cycle_batch()?;
            let after = self.quiescence_status();
            if after == before {
                return Ok(after);
            }
        }
    }

    pub fn quiescence_status(&self) -> QuiescenceStatus {
        let info = self
            .inner
            .control()
            .runtime(self.world_id)
            .expect("world runtime should exist while harness is open");
        let kernel_idle = !info.has_pending_inbox;
        let runtime_quiescent = kernel_idle
            && !info.has_pending_effects
            && info.next_timer_due_at_ns.is_none()
            && !info.has_pending_maintenance;
        QuiescenceStatus {
            kernel: KernelQuiescenceStatus {
                kernel_idle,
                runtime_quiescent,
                workflow_queue_pending: info.has_pending_inbox,
                queued_effects: usize::from(info.has_pending_effects),
                pending_workflow_receipts: 0,
                inflight_workflow_intents: 0,
                non_terminal_workflow_instances: 0,
            },
            runtime_quiescent,
            timers_pending: usize::from(info.next_timer_due_at_ns.is_some()),
            next_timer_deadline_ns: info.next_timer_due_at_ns,
        }
    }

    pub fn pull_effects(&mut self) -> Result<Vec<EffectIntent>> {
        bail!(
            "runtime-backed WorldHarness does not expose pull_effects(); use WorkflowHarness for scripted effect choreography"
        )
    }

    pub fn apply_receipt(&mut self, _receipt: EffectReceipt) -> Result<()> {
        bail!(
            "runtime-backed WorldHarness does not support apply_receipt(); use WorkflowHarness for manual receipt injection"
        )
    }

    pub fn snapshot(&mut self) -> Result<()> {
        self.inner.control().checkpoint_world(self.world_id)?;
        Ok(())
    }

    pub fn trace_summary(&self) -> Result<JsonValue> {
        Ok(self.inner.control().trace_summary(self.world_id, 256)?)
    }

    pub fn time_get(&self) -> u64 {
        0
    }

    pub fn time_set(&mut self, _now_ns: u64) -> u64 {
        0
    }

    pub fn time_advance(&mut self, _delta_ns: u64) -> u64 {
        0
    }

    pub fn time_jump_next_due(&mut self) -> Result<Option<u64>> {
        bail!("runtime-backed WorldHarness does not support logical time control")
    }

    pub fn export_artifacts(&self) -> Result<HarnessArtifacts> {
        let mut from = 0u64;
        let mut journal_entries = Vec::new();
        loop {
            let page = self
                .inner
                .control()
                .journal_entries(self.world_id, from, 1024)
                .context("load world journal entries")?;
            if page.entries.is_empty() {
                break;
            }
            journal_entries.extend(page.entries.into_iter().map(|entry| entry.record));
            if page.next_from <= from {
                break;
            }
            from = page.next_from;
        }
        Ok(HarnessArtifacts {
            journal_entries,
            trace_summary: self.trace_summary()?,
            evidence: HarnessEvidence {
                cycles_run: self.cycles_run,
            },
        })
    }

    pub fn receipt_ok(&self, intent_hash: [u8; 32], payload: &JsonValue) -> Result<EffectReceipt> {
        Ok(EffectReceipt {
            intent_hash,
            status: ReceiptStatus::Ok,
            payload_cbor: to_canonical_cbor(payload)?,
            cost_cents: None,
            signature: Vec::new(),
        })
    }

    pub fn receipt_error(
        &self,
        intent_hash: [u8; 32],
        payload: &JsonValue,
    ) -> Result<EffectReceipt> {
        Ok(EffectReceipt {
            intent_hash,
            status: ReceiptStatus::Error,
            payload_cbor: to_canonical_cbor(payload)?,
            cost_cents: None,
            signature: Vec::new(),
        })
    }

    pub fn receipt_timeout(
        &self,
        intent_hash: [u8; 32],
        payload: &JsonValue,
    ) -> Result<EffectReceipt> {
        Ok(EffectReceipt {
            intent_hash,
            status: ReceiptStatus::Timeout,
            payload_cbor: to_canonical_cbor(payload)?,
            cost_cents: None,
            signature: Vec::new(),
        })
    }

    pub fn receipt_timer_set_ok(
        &self,
        intent_hash: [u8; 32],
        delivered_at_ns: u64,
        key: Option<String>,
    ) -> Result<EffectReceipt> {
        self.receipt_ok(
            intent_hash,
            &serde_json::to_value(TimerSetReceipt {
                delivered_at_ns,
                key,
            })?,
        )
    }

    pub fn receipt_blob_put_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &BlobPutReceipt,
    ) -> Result<EffectReceipt> {
        self.receipt_ok(intent_hash, &serde_json::to_value(payload)?)
    }

    pub fn receipt_blob_get_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &BlobGetReceipt,
    ) -> Result<EffectReceipt> {
        self.receipt_ok(intent_hash, &serde_json::to_value(payload)?)
    }

    pub fn receipt_llm_generate_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &LlmGenerateReceipt,
    ) -> Result<EffectReceipt> {
        self.receipt_ok(intent_hash, &serde_json::to_value(payload)?)
    }

    pub fn state_json(&self, workflow: &str, key: Option<&[u8]>) -> Result<JsonValue> {
        match self.state_bytes(workflow, key)? {
            Some(bytes) => decode_state_json(&bytes),
            None => Ok(JsonValue::Null),
        }
    }

    pub fn state_bytes(&self, workflow: &str, key: Option<&[u8]>) -> Result<Option<Vec<u8>>> {
        let response = self
            .inner
            .control()
            .state_get(
                self.world_id,
                workflow,
                key.map(|bytes| bytes.to_vec()),
                Some("latest_durable"),
            )
            .context("load world state")?;
        response
            .state_b64
            .map(|value| {
                BASE64_STANDARD
                    .decode(value)
                    .context("decode state bytes from base64")
            })
            .transpose()
    }

    pub fn list_cells(&self, workflow: &str) -> Result<Vec<HarnessCell>> {
        let response = self
            .inner
            .control()
            .state_list(self.world_id, workflow, u32::MAX, Some("latest_durable"))
            .context("list world cells")?;
        Ok(response
            .cells
            .iter()
            .map(HarnessCell::from_state_cell)
            .collect())
    }

    pub fn blob_bytes(&self, blob_ref: &str) -> Result<Vec<u8>> {
        let hash = Hash::from_hex_str(blob_ref)
            .map_err(|err| anyhow!("invalid blob ref '{blob_ref}': {err}"))?;
        self.inner
            .control()
            .get_blob(hash)
            .context("load world blob")
    }

    pub fn reopen(&self) -> Result<Self> {
        Ok(Self {
            inner: self.inner.reopen().context("reopen node world harness")?,
            world_id: self.world_id,
            cycles_run: self.cycles_run,
        })
    }
}

impl RuntimeWorkflowHarness<MemStore> {
    pub fn send_event(&mut self, schema: &str, json_value: JsonValue) -> Result<()> {
        self.inner.send_event(schema, json_value)
    }

    pub fn send_command(&mut self, schema: &str, json_value: JsonValue) -> Result<()> {
        self.inner.send_command(schema, json_value)
    }

    pub fn run_cycle_batch(&mut self) -> Result<CycleOutcome> {
        self.inner.run_cycle(false)
    }

    pub fn run_until_kernel_idle(&mut self) -> Result<QuiescenceStatus> {
        self.inner.run_until_kernel_idle()
    }

    pub fn run_until_runtime_quiescent(&mut self) -> Result<QuiescenceStatus> {
        self.inner.run_until_runtime_quiescent()
    }

    pub fn quiescence_status(&self) -> QuiescenceStatus {
        self.inner.quiescence_status()
    }

    pub fn pull_effects(&mut self) -> Result<Vec<EffectIntent>> {
        self.inner.pull_effects()
    }

    pub fn apply_receipt(&mut self, receipt: EffectReceipt) -> Result<()> {
        self.inner.apply_receipt(receipt)
    }

    pub fn snapshot(&mut self) -> Result<()> {
        self.inner.snapshot()
    }

    pub fn trace_summary(&self) -> Result<JsonValue> {
        self.inner.trace_summary()
    }

    pub fn time_get(&self) -> u64 {
        self.inner.time_get()
    }

    pub fn time_set(&mut self, now_ns: u64) -> u64 {
        self.inner.time_set(now_ns)
    }

    pub fn time_advance(&mut self, delta_ns: u64) -> u64 {
        self.inner.time_advance(delta_ns)
    }

    pub fn time_jump_next_due(&mut self) -> Result<Option<u64>> {
        self.inner.time_jump_next_due()
    }

    pub fn export_artifacts(&self) -> Result<HarnessArtifacts> {
        self.inner.export_artifacts()
    }

    pub fn receipt_ok(&self, intent_hash: [u8; 32], payload: &JsonValue) -> Result<EffectReceipt> {
        self.inner.receipt_ok(intent_hash, payload)
    }

    pub fn receipt_error(
        &self,
        intent_hash: [u8; 32],
        payload: &JsonValue,
    ) -> Result<EffectReceipt> {
        self.inner.receipt_error(intent_hash, payload)
    }

    pub fn receipt_timeout(
        &self,
        intent_hash: [u8; 32],
        payload: &JsonValue,
    ) -> Result<EffectReceipt> {
        self.inner.receipt_timeout(intent_hash, payload)
    }

    pub fn receipt_timer_set_ok(
        &self,
        intent_hash: [u8; 32],
        delivered_at_ns: u64,
        key: Option<String>,
    ) -> Result<EffectReceipt> {
        self.inner
            .receipt_timer_set_ok(intent_hash, delivered_at_ns, key)
    }

    pub fn receipt_blob_put_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &BlobPutReceipt,
    ) -> Result<EffectReceipt> {
        self.inner.receipt_blob_put_ok(intent_hash, payload)
    }

    pub fn receipt_blob_get_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &BlobGetReceipt,
    ) -> Result<EffectReceipt> {
        self.inner.receipt_blob_get_ok(intent_hash, payload)
    }

    pub fn receipt_llm_generate_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &LlmGenerateReceipt,
    ) -> Result<EffectReceipt> {
        self.inner.receipt_llm_generate_ok(intent_hash, payload)
    }

    pub fn state_json(&self, key: Option<&[u8]>) -> Result<JsonValue> {
        self.inner.state_json(&self.workflow, key)
    }

    pub fn state_bytes(&self, key: Option<&[u8]>) -> Option<Vec<u8>> {
        self.inner.state_bytes(&self.workflow, key)
    }

    pub fn list_cells(&self) -> Result<Vec<HarnessCell>> {
        self.inner.list_cells(&self.workflow)
    }

    pub fn blob_bytes(&self, blob_ref: &str) -> Result<Vec<u8>> {
        self.inner.blob_bytes(blob_ref)
    }

    pub fn reopen(&self) -> Result<Self> {
        Ok(Self {
            workflow: self.workflow.clone(),
            inner: self.inner.reopen()?,
        })
    }
}

fn decode_state_json(bytes: &[u8]) -> Result<JsonValue> {
    let value: serde_cbor::Value =
        serde_cbor::from_slice(bytes).context("decode workflow state cbor")?;
    serde_json::to_value(value).context("encode workflow state json")
}

fn classify_effect_intent(intent: &EffectIntent) -> EffectExecutionClass {
    if INTERNAL_EFFECT_KINDS.contains(&intent.effect.as_str()) {
        return EffectExecutionClass::InlineInternal;
    }
    if intent.effect.as_str() == effect_ops::TIMER_SET {
        return EffectExecutionClass::OwnerLocalTimer;
    }
    EffectExecutionClass::ExternalAsync
}

const INTERNAL_EFFECT_KINDS: &[&str] = &[
    effect_ops::INTROSPECT_MANIFEST,
    effect_ops::INTROSPECT_WORKFLOW_STATE,
    effect_ops::INTROSPECT_JOURNAL_HEAD,
    effect_ops::INTROSPECT_LIST_CELLS,
    effect_ops::WORKSPACE_RESOLVE,
    effect_ops::WORKSPACE_EMPTY_ROOT,
    effect_ops::WORKSPACE_LIST,
    effect_ops::WORKSPACE_READ_REF,
    effect_ops::WORKSPACE_READ_BYTES,
    effect_ops::WORKSPACE_WRITE_BYTES,
    effect_ops::WORKSPACE_WRITE_REF,
    effect_ops::WORKSPACE_REMOVE,
    effect_ops::WORKSPACE_DIFF,
    effect_ops::WORKSPACE_ANNOTATIONS_GET,
    effect_ops::WORKSPACE_ANNOTATIONS_SET,
    "sys/governance.propose@1",
    "sys/governance.shadow@1",
    "sys/governance.approve@1",
    "sys/governance.apply@1",
];
