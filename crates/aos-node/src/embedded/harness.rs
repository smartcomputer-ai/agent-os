use std::sync::Arc;

use aos_effect_adapters::config::EffectAdapterConfig;
use aos_effects::builtins::{
    BlobGetReceipt, BlobPutReceipt, HttpRequestReceipt, LlmGenerateReceipt, RequestTimings,
    TimerSetReceipt,
};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use aos_kernel::cell_index::CellMeta;
use aos_kernel::journal::{Journal, SnapshotRecord as KernelSnapshotRecord};
use aos_kernel::{Kernel, KernelConfig};
use aos_runtime::timer::TimerScheduler;
use aos_runtime::{
    CycleOutcome, EffectMode, HarnessArtifacts, HarnessEvidence, HarnessReplayReport, HostError,
    JournalReplayOpen, QuiescenceStatus, RunMode, WorldConfig, WorldHost, now_wallclock_ns,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};
use uuid::Uuid;

use crate::{
    CborPayload, CreateWorldRequest, CreateWorldSource, DomainEventIngress, ReceiptIngress,
    UniverseId, WorldId,
};

use super::{FsCas, LocalKernelGuard, LocalLogRuntime, LocalRuntimeError, LocalStatePaths};

const LOCAL_UNIVERSE_ID: &str = "00000000-0000-0000-0000-000000000000";

pub fn local_universe_id() -> UniverseId {
    UniverseId::from(Uuid::parse_str(LOCAL_UNIVERSE_ID).expect("valid singleton universe id"))
}

pub struct EmbeddedWorldHarness {
    paths: LocalStatePaths,
    runtime: Arc<LocalLogRuntime>,
    world_id: WorldId,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: KernelConfig,
    effect_mode: EffectMode,
    exec: Runtime,
    timer_scheduler: TimerScheduler,
    cycles_run: u64,
    effects_dispatched: u64,
    receipts_applied: u64,
}

impl EmbeddedWorldHarness {
    pub fn bootstrap(
        paths: LocalStatePaths,
        world_id: WorldId,
        manifest_hash: String,
        world_config: WorldConfig,
        adapter_config: EffectAdapterConfig,
        kernel_config: KernelConfig,
        effect_mode: EffectMode,
    ) -> Result<Self, HostError> {
        let runtime = LocalLogRuntime::open_with_config(
            paths.clone(),
            world_config.clone(),
            adapter_config.clone(),
            kernel_config.clone(),
        )
        .map_err(host_error_from_local)?;
        runtime
            .create_world(CreateWorldRequest {
                world_id: Some(world_id),
                universe_id: crate::UniverseId::nil(),
                created_at_ns: now_wallclock_ns(),
                source: CreateWorldSource::Manifest { manifest_hash },
            })
            .map_err(host_error_from_local)?;
        Self::from_runtime(
            paths,
            runtime,
            world_id,
            world_config,
            adapter_config,
            kernel_config,
            effect_mode,
        )
    }

    pub fn from_runtime(
        paths: LocalStatePaths,
        runtime: Arc<LocalLogRuntime>,
        world_id: WorldId,
        world_config: WorldConfig,
        adapter_config: EffectAdapterConfig,
        kernel_config: KernelConfig,
        effect_mode: EffectMode,
    ) -> Result<Self, HostError> {
        let exec = RuntimeBuilder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| HostError::External(format!("build embedded harness runtime: {err}")))?;
        Ok(Self {
            paths,
            runtime,
            world_id,
            world_config,
            adapter_config,
            kernel_config,
            effect_mode,
            exec,
            timer_scheduler: TimerScheduler::new(),
            cycles_run: 0,
            effects_dispatched: 0,
            receipts_applied: 0,
        })
    }

    pub fn world_id(&self) -> WorldId {
        self.world_id
    }

    pub fn store(&self) -> Arc<FsCas> {
        self.runtime.store()
    }

    pub fn send_event(&mut self, schema: &str, json_value: JsonValue) -> Result<(), HostError> {
        let cbor =
            serde_cbor::to_vec(&json_value).map_err(|err| HostError::External(err.to_string()))?;
        self.send_event_cbor(schema, cbor)
    }

    pub fn send_event_cbor(&mut self, schema: &str, value: Vec<u8>) -> Result<(), HostError> {
        self.runtime
            .enqueue_event(
                self.world_id,
                DomainEventIngress {
                    schema: schema.to_string(),
                    value: CborPayload::inline(value),
                    key: None,
                    correlation_id: None,
                },
            )
            .map_err(host_error_from_local)?;
        Ok(())
    }

    pub fn send_command(&mut self, schema: &str, json_value: JsonValue) -> Result<(), HostError> {
        self.send_event(schema, json_value)
    }

    pub fn run_until_kernel_idle(&mut self) -> Result<QuiescenceStatus, HostError> {
        self.runtime
            .mutate_world_host(self.world_id, |host| {
                host.drain()?;
                Ok(())
            })
            .map_err(host_error_from_local)?;
        Ok(self.quiescence_status()?)
    }

    pub fn run_until_runtime_quiescent(&mut self) -> Result<QuiescenceStatus, HostError> {
        self.run_until_kernel_idle()
    }

    pub fn run_cycle_batch(&mut self) -> Result<CycleOutcome, HostError> {
        let exec = &self.exec;
        let cycle = self
            .runtime
            .mutate_world_host(self.world_id, |host| {
                exec.block_on(host.run_cycle(RunMode::Batch))
            })
            .map_err(host_error_from_local)?;
        self.record_cycle(cycle);
        Ok(cycle)
    }

    pub fn run_cycle_with_timers(&mut self) -> Result<CycleOutcome, HostError> {
        let exec = &self.exec;
        let scheduler = &mut self.timer_scheduler;
        let mut cycle = self
            .runtime
            .mutate_world_host(self.world_id, |host| {
                exec.block_on(host.run_cycle(RunMode::Daemon { scheduler }))
            })
            .map_err(host_error_from_local)?;
        let fired = self
            .runtime
            .mutate_world_host(self.world_id, |host| {
                host.fire_due_timers(&mut self.timer_scheduler)
            })
            .map_err(host_error_from_local)?;
        if fired > 0 {
            self.runtime
                .mutate_world_host(self.world_id, |host| host.drain())
                .map_err(host_error_from_local)?;
            cycle.receipts_applied += fired;
        }
        self.record_cycle(cycle);
        Ok(cycle)
    }

    pub fn quiescence_status(&self) -> Result<QuiescenceStatus, HostError> {
        self.runtime
            .inspect_world_host(self.world_id, |host| {
                Ok(host.quiescence_status(Some(&self.timer_scheduler)))
            })
            .map_err(host_error_from_local)
    }

    pub fn pull_effects(&mut self) -> Result<Vec<EffectIntent>, HostError> {
        self.runtime
            .mutate_world_host(self.world_id, |host| {
                host.kernel_mut().drain_effects().map_err(HostError::from)
            })
            .map_err(host_error_from_local)
    }

    pub fn apply_receipt(&mut self, receipt: EffectReceipt) -> Result<(), HostError> {
        self.runtime
            .enqueue_receipt(
                self.world_id,
                ReceiptIngress {
                    intent_hash: receipt.intent_hash.to_vec(),
                    effect_kind: receipt.adapter_id.clone(),
                    adapter_id: receipt.adapter_id,
                    status: receipt.status,
                    payload: CborPayload::inline(receipt.payload_cbor),
                    cost_cents: receipt.cost_cents,
                    signature: receipt.signature,
                    correlation_id: None,
                },
            )
            .map_err(host_error_from_local)?;
        Ok(())
    }

    pub fn snapshot(&mut self) -> Result<(), HostError> {
        self.runtime
            .mutate_world_host(self.world_id, |host| host.snapshot())
            .map_err(host_error_from_local)?;
        Ok(())
    }

    pub fn reopen(&self) -> Result<Self, HostError> {
        let reopened = Self::from_runtime(
            self.paths.clone(),
            LocalLogRuntime::open_with_config(
                self.paths.clone(),
                self.world_config.clone(),
                self.adapter_config.clone(),
                self.kernel_config.clone(),
            )
            .map_err(host_error_from_local)?,
            self.world_id,
            self.world_config.clone(),
            self.adapter_config.clone(),
            self.kernel_config.clone(),
            self.effect_mode,
        )?;
        Ok(reopened)
    }

    pub fn replay_check(&self) -> Result<HarnessReplayReport, HostError> {
        let reopened = self.replay_open_world_host()?;
        let mut mismatches = Vec::new();
        if self.journal_entries()? != reopened.kernel().dump_journal().map_err(HostError::from)? {
            mismatches.push("journal entries".into());
        }
        Ok(HarnessReplayReport {
            ok: mismatches.is_empty(),
            mismatches,
        })
    }

    pub fn replay_state_bytes(
        &self,
        workflow: &str,
        key: Option<&[u8]>,
    ) -> Result<Option<Vec<u8>>, HostError> {
        Ok(self.replay_open_world_host()?.state(workflow, key))
    }

    pub fn state_bytes(&self, workflow: &str, key: Option<&[u8]>) -> Option<Vec<u8>> {
        self.runtime
            .inspect_world_host(self.world_id, |host| Ok(host.state(workflow, key)))
            .ok()
            .flatten()
    }

    pub fn state<T: DeserializeOwned>(
        &self,
        workflow: &str,
        key: Option<&[u8]>,
    ) -> Result<T, HostError> {
        let bytes = self.state_bytes(workflow, key).ok_or_else(|| {
            HostError::External(format!("missing state for workflow '{workflow}'"))
        })?;
        serde_cbor::from_slice(&bytes).map_err(|err| HostError::External(err.to_string()))
    }

    pub fn state_json(&self, workflow: &str, key: Option<&[u8]>) -> Result<JsonValue, HostError> {
        let bytes = self.state_bytes(workflow, key).ok_or_else(|| {
            HostError::External(format!("missing state for workflow '{workflow}'"))
        })?;
        let value: serde_cbor::Value =
            serde_cbor::from_slice(&bytes).map_err(|err| HostError::External(err.to_string()))?;
        serde_json::to_value(value).map_err(|err| HostError::External(err.to_string()))
    }

    pub fn list_cells(&self, workflow: &str) -> Result<Vec<CellMeta>, HostError> {
        Ok(self
            .runtime
            .inspect_world_host(self.world_id, |host| host.list_cells(workflow))
            .map_err(host_error_from_local)?)
    }

    pub fn trace_summary(&self) -> Result<JsonValue, HostError> {
        Ok(self
            .runtime
            .inspect_world_host(self.world_id, |host| host.trace_summary())
            .map_err(host_error_from_local)?)
    }

    pub fn time_get(&self) -> Result<u64, HostError> {
        self.runtime
            .inspect_world_host(self.world_id, |host| Ok(host.logical_time_now_ns()))
            .map_err(host_error_from_local)
    }

    pub fn time_set(&mut self, now_ns: u64) -> Result<u64, HostError> {
        self.runtime
            .mutate_world_host(self.world_id, |host| Ok(host.set_logical_time_ns(now_ns)))
            .map_err(host_error_from_local)
    }

    pub fn time_advance(&mut self, delta_ns: u64) -> Result<u64, HostError> {
        self.runtime
            .mutate_world_host(self.world_id, |host| {
                Ok(host.advance_logical_time_ns(delta_ns))
            })
            .map_err(host_error_from_local)
    }

    pub fn time_jump_next_due(&mut self) -> Result<Option<u64>, HostError> {
        let Some(next_due_at_ns) = self.timer_scheduler.next_due_at_ns() else {
            return Ok(None);
        };
        let _ = self.time_set(next_due_at_ns)?;
        let fired = self
            .runtime
            .mutate_world_host(self.world_id, |host| {
                host.fire_due_timers(&mut self.timer_scheduler)
            })
            .map_err(host_error_from_local)?;
        if fired > 0 {
            self.runtime
                .mutate_world_host(self.world_id, |host| {
                    host.drain()?;
                    Ok(())
                })
                .map_err(host_error_from_local)?;
            self.receipts_applied += fired as u64;
        }
        Ok(Some(next_due_at_ns))
    }

    pub fn evidence(&self) -> Result<HarnessEvidence, HostError> {
        let summary = self
            .runtime
            .world_summary(self.world_id)
            .map_err(host_error_from_local)?;
        let logical_time_ns = self.time_get()?;
        let quiescence = self.quiescence_status()?;
        Ok(HarnessEvidence {
            backend: "embedded-local",
            effect_mode: match self.effect_mode {
                EffectMode::Scripted => "scripted",
                EffectMode::Twin => "twin",
                EffectMode::Live => "live",
            },
            logical_time_ns,
            heights_head: summary.0.notify_counter,
            heights_snapshot: Some(summary.1.height),
            cycles_run: self.cycles_run,
            effects_dispatched: self.effects_dispatched,
            receipts_applied: self.receipts_applied,
            quiescence,
        })
    }

    pub fn export_artifacts(&self) -> Result<HarnessArtifacts, HostError> {
        Ok(HarnessArtifacts {
            evidence: self.evidence()?,
            trace_summary: self.trace_summary()?,
            journal_entries: self.journal_entries()?,
        })
    }

    pub fn receipt_ok<T: Serialize>(
        &self,
        intent_hash: [u8; 32],
        adapter_id: impl Into<String>,
        payload: &T,
    ) -> Result<EffectReceipt, HostError> {
        build_receipt(intent_hash, adapter_id.into(), ReceiptStatus::Ok, payload)
    }

    pub fn receipt_error<T: Serialize>(
        &self,
        intent_hash: [u8; 32],
        adapter_id: impl Into<String>,
        payload: &T,
    ) -> Result<EffectReceipt, HostError> {
        build_receipt(
            intent_hash,
            adapter_id.into(),
            ReceiptStatus::Error,
            payload,
        )
    }

    pub fn receipt_timeout<T: Serialize>(
        &self,
        intent_hash: [u8; 32],
        adapter_id: impl Into<String>,
        payload: &T,
    ) -> Result<EffectReceipt, HostError> {
        build_receipt(
            intent_hash,
            adapter_id.into(),
            ReceiptStatus::Timeout,
            payload,
        )
    }

    pub fn receipt_timer_set_ok(
        &self,
        intent_hash: [u8; 32],
        delivered_at_ns: u64,
        key: Option<String>,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_ok(
            intent_hash,
            "adapter.timer.harness",
            &TimerSetReceipt {
                delivered_at_ns,
                key,
            },
        )
    }

    pub fn receipt_blob_put_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &BlobPutReceipt,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_ok(intent_hash, "adapter.blob.put.harness", payload)
    }

    pub fn receipt_blob_get_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &BlobGetReceipt,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_ok(intent_hash, "adapter.blob.get.harness", payload)
    }

    pub fn receipt_http_request_ok(
        &self,
        intent_hash: [u8; 32],
        status: i32,
        adapter_id: impl Into<String>,
    ) -> Result<EffectReceipt, HostError> {
        let adapter_id = adapter_id.into();
        self.receipt_ok(
            intent_hash,
            adapter_id.clone(),
            &HttpRequestReceipt {
                status,
                headers: Default::default(),
                body_ref: None,
                timings: RequestTimings {
                    start_ns: self.time_get()?,
                    end_ns: self.time_get()?,
                },
                adapter_id,
            },
        )
    }

    pub fn receipt_llm_generate_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &LlmGenerateReceipt,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_ok(intent_hash, payload.provider_id.clone(), payload)
    }

    pub fn execute_batch_routed(
        &mut self,
        intents: Vec<(EffectIntent, String)>,
    ) -> Result<Vec<EffectReceipt>, HostError> {
        let exec = &self.exec;
        self.runtime
            .mutate_world_host(self.world_id, |host| {
                Ok(exec.block_on(host.adapter_registry_mut().execute_batch_routed(intents)))
            })
            .map_err(host_error_from_local)
    }

    pub fn execute_single_routed(
        &mut self,
        intent: EffectIntent,
        route_id: String,
    ) -> Result<EffectReceipt, HostError> {
        let mut receipts = self.execute_batch_routed(vec![(intent, route_id)])?;
        receipts
            .drain(..)
            .next()
            .ok_or_else(|| HostError::External("missing adapter receipt".into()))
    }

    pub fn with_kernel_mut<R>(
        &mut self,
        mutate: impl FnOnce(&mut Kernel<FsCas>) -> Result<R, aos_kernel::KernelError>,
    ) -> Result<R, HostError> {
        self.runtime
            .mutate_world_host(self.world_id, |host| {
                mutate(host.kernel_mut()).map_err(HostError::from)
            })
            .map_err(host_error_from_local)
    }

    pub fn kernel_mut(&mut self) -> Result<LocalKernelGuard<'_>, HostError> {
        self.runtime
            .kernel_mut(self.world_id)
            .map_err(host_error_from_local)
    }

    pub fn with_kernel<R>(
        &self,
        inspect: impl FnOnce(&Kernel<FsCas>) -> Result<R, HostError>,
    ) -> Result<R, HostError> {
        Ok(self
            .runtime
            .inspect_world_host(self.world_id, |host| inspect(host.kernel()))
            .map_err(host_error_from_local)?)
    }

    fn journal_entries(&self) -> Result<Vec<aos_kernel::journal::OwnedJournalEntry>, HostError> {
        Ok(self
            .runtime
            .inspect_world_host(self.world_id, |host| {
                host.kernel().dump_journal().map_err(HostError::from)
            })
            .map_err(host_error_from_local)?)
    }

    fn replay_open_world_host(&self) -> Result<WorldHost<FsCas>, HostError> {
        let store = self.store();
        let manifest_hash = self
            .runtime
            .inspect_world_host(self.world_id, |host| Ok(host.kernel().manifest_hash()))
            .map_err(host_error_from_local)?;
        let retained = self.journal_entries()?;
        let (_, active_baseline) = self
            .runtime
            .world_summary(self.world_id)
            .map_err(host_error_from_local)?;
        let loaded = aos_kernel::ManifestLoader::load_from_hash(store.as_ref(), manifest_hash)
            .map_err(HostError::from)?;
        let replay = JournalReplayOpen {
            active_baseline: KernelSnapshotRecord {
                snapshot_ref: active_baseline.snapshot_ref,
                height: active_baseline.height,
                universe_id: active_baseline.universe_id.as_uuid(),
                logical_time_ns: active_baseline.logical_time_ns,
                receipt_horizon_height: active_baseline.receipt_horizon_height,
                manifest_hash: active_baseline.manifest_hash,
            },
            replay_seed: None,
        };
        WorldHost::from_loaded_manifest_with_journal_replay(
            store,
            loaded,
            Journal::from_entries(&retained).map_err(|err| HostError::External(err.to_string()))?,
            self.world_config.clone(),
            self.adapter_config.clone(),
            self.kernel_config.clone(),
            Some(replay),
        )
    }

    fn record_cycle(&mut self, cycle: CycleOutcome) {
        self.cycles_run += 1;
        self.effects_dispatched += cycle.effects_dispatched as u64;
        self.receipts_applied += cycle.receipts_applied as u64;
    }
}

fn build_receipt<T: Serialize>(
    intent_hash: [u8; 32],
    adapter_id: String,
    status: ReceiptStatus,
    payload: &T,
) -> Result<EffectReceipt, HostError> {
    Ok(EffectReceipt {
        intent_hash,
        adapter_id,
        status,
        payload_cbor: serde_cbor::to_vec(payload)
            .map_err(|err| HostError::External(format!("encode receipt payload: {err}")))?,
        cost_cents: Some(0),
        signature: vec![],
    })
}

fn host_error_from_local(err: LocalRuntimeError) -> HostError {
    match err {
        LocalRuntimeError::Kernel(err) => HostError::Kernel(err),
        LocalRuntimeError::Store(err) => HostError::Store(err.to_string()),
        LocalRuntimeError::Host(err) => err,
        other => HostError::External(other.to_string()),
    }
}
