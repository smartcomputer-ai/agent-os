use std::sync::Arc;

use aos_cbor::Hash;
use aos_kernel::{LoadedManifest, Store};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;

use crate::blobstore::BlobStoreConfig;
use crate::bootstrap::build_control_deps_from_worker_runtime;
use crate::control::ControlFacade;
use crate::control::ForkWorldBody;
use crate::control::{
    AcceptWaitQuery, CommandSubmitResponse, HeadInfoResponse, JournalEntriesResponse, JournalQuery,
    LimitQuery, ManifestResponse, PutSecretVersionBody, StateGetQuery, StateListResponse,
    UpsertSecretBindingBody,
};
use crate::{
    CborPayload, CommandRecord, CreateWorldRequest, CreateWorldSource, DomainEventIngress,
    ForkWorldRequest, HostedWorkerRuntime, InboxSeq, LocalStatePaths, SnapshotRecord, UniverseId,
    WorldId, WorldRecord, WorldRuntimeInfo,
};

pub type NodeHarnessError = anyhow::Error;

#[derive(Clone)]
pub struct NodeWorldHarness {
    paths: LocalStatePaths,
    runtime: HostedWorkerRuntime,
    control: Arc<NodeHarnessControl>,
}

pub struct NodeHarnessControl {
    runtime: HostedWorkerRuntime,
    facade: ControlFacade,
}

#[derive(Debug, Clone)]
pub struct NodeHarnessStep {
    pub runtime: WorldRuntimeInfo,
    pub active_baseline: SnapshotRecord,
}

#[derive(Debug, Clone)]
pub struct NodeForkResult {
    pub record: WorldRecord,
}

impl NodeWorldHarness {
    pub fn open(state_root: &std::path::Path) -> Result<Self, NodeHarnessError> {
        let paths = LocalStatePaths::new(state_root.to_path_buf());
        paths.ensure_root()?;
        let runtime = HostedWorkerRuntime::new_sqlite_with_state_root_and_universe(
            paths.root(),
            UniverseId::nil(),
            BlobStoreConfig::default(),
        )?;
        let facade = ControlFacade::new(build_control_deps_from_worker_runtime(runtime.clone())?)?;
        Ok(Self {
            paths,
            runtime: runtime.clone(),
            control: Arc::new(NodeHarnessControl { runtime, facade }),
        })
    }

    pub fn reopen(&self) -> Result<Self, NodeHarnessError> {
        Self::open(self.paths.root())
    }

    pub fn paths(&self) -> &LocalStatePaths {
        &self.paths
    }

    pub fn control(&self) -> &NodeHarnessControl {
        &self.control
    }

    pub fn create_world_from_loaded_manifest<S: Store + ?Sized>(
        &self,
        source_store: &S,
        loaded: &LoadedManifest,
        world_id: WorldId,
        created_at_ns: u64,
    ) -> Result<(), NodeHarnessError> {
        let universe_id = UniverseId::nil();
        let store = self.runtime.cas_store_for_domain(universe_id)?;
        for schema in loaded.schemas.values() {
            store.put_node(schema)?;
        }
        for module in loaded.modules.values() {
            store.put_node(module)?;
            let wasm_hash = Hash::from_hex_str(module.wasm_hash.as_str())?;
            if let Ok(bytes) = source_store.get_blob(wasm_hash) {
                store.put_blob(&bytes)?;
            }
        }
        for effect in loaded.effects.values() {
            store.put_node(effect)?;
        }
        for cap in loaded.caps.values() {
            store.put_node(cap)?;
        }
        for policy in loaded.policies.values() {
            store.put_node(policy)?;
        }
        let manifest_hash = store.put_node(&loaded.manifest)?.to_hex();
        self.runtime.create_world(
            universe_id,
            CreateWorldRequest {
                world_id: Some(world_id),
                universe_id,
                created_at_ns,
                source: CreateWorldSource::Manifest { manifest_hash },
            },
        )?;
        Ok(())
    }
}

impl NodeHarnessControl {
    pub fn enqueue_event(
        &self,
        world_id: WorldId,
        ingress: DomainEventIngress,
    ) -> Result<InboxSeq, NodeHarnessError> {
        let value: serde_json::Value = match ingress.value {
            CborPayload {
                inline_cbor: Some(bytes),
                ..
            } => serde_cbor::from_slice(&bytes)?,
            _ => anyhow::bail!("node harness requires inline CBOR event payloads"),
        };
        self.runtime.submit_event_with_wait(
            crate::SubmitEventRequest {
                universe_id: UniverseId::nil(),
                world_id,
                schema: ingress.schema,
                value,
                submission_id: ingress.correlation_id,
                expected_world_epoch: None,
            },
            AcceptWaitQuery {
                wait_for_flush: true,
                wait_timeout_ms: None,
            },
        )?;
        Ok(InboxSeq::from_u64(0))
    }

    pub fn submit_command<T: serde::Serialize>(
        &self,
        world_id: WorldId,
        command: &str,
        command_id: Option<String>,
        actor: Option<String>,
        params: &T,
    ) -> Result<CommandSubmitResponse, NodeHarnessError> {
        Ok(self.runtime.submit_command_with_wait(
            UniverseId::nil(),
            world_id,
            command,
            command_id,
            actor,
            params,
            AcceptWaitQuery {
                wait_for_flush: true,
                wait_timeout_ms: None,
            },
        )?)
    }

    pub fn get_command(
        &self,
        world_id: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, NodeHarnessError> {
        Ok(self.facade.get_command(world_id, command_id)?)
    }

    pub fn put_node_secret(
        &self,
        binding_id: &str,
        plaintext: &[u8],
    ) -> Result<(), NodeHarnessError> {
        self.facade.upsert_secret_binding(
            UniverseId::nil(),
            binding_id,
            UpsertSecretBindingBody {
                source_kind: crate::SecretBindingSourceKind::NodeSecretStore,
                env_var: None,
                required_placement_pin: None,
                status: crate::SecretBindingStatus::Active,
                actor: Some("aos-node-harness".to_owned()),
            },
        )?;
        self.facade.put_secret_version(
            UniverseId::nil(),
            binding_id,
            PutSecretVersionBody {
                plaintext_b64: BASE64_STANDARD.encode(plaintext),
                expected_digest: None,
                actor: Some("aos-node-harness".to_owned()),
            },
        )?;
        Ok(())
    }

    pub fn step_world(&self, world_id: WorldId) -> Result<NodeHarnessStep, NodeHarnessError> {
        let _ = self.runtime.runtime_info(UniverseId::nil(), world_id)?;
        self.runtime.drive_until_quiescent(true)?;
        self.world_summary(world_id)
    }

    pub fn checkpoint_world(&self, world_id: WorldId) -> Result<NodeHarnessStep, NodeHarnessError> {
        self.runtime
            .checkpoint_world_now(UniverseId::nil(), world_id)?;
        self.world_summary(world_id)
    }

    pub fn fork_world(
        &self,
        request: ForkWorldRequest,
    ) -> Result<NodeForkResult, NodeHarnessError> {
        let accepted = self.facade.fork_world(
            request.src_world_id,
            ForkWorldBody {
                src_snapshot: request.src_snapshot,
                new_world_id: request.new_world_id,
                forked_at_ns: request.forked_at_ns,
                pending_effect_policy: request.pending_effect_policy,
            },
        )?;
        let manifest = self
            .runtime
            .manifest(UniverseId::nil(), accepted.world_id)?;
        let active_baseline = self
            .runtime
            .active_baseline(UniverseId::nil(), accepted.world_id)?;
        let head = self
            .runtime
            .journal_head(UniverseId::nil(), accepted.world_id)?
            .journal_head;
        Ok(NodeForkResult {
            record: WorldRecord {
                world_id: accepted.world_id,
                universe_id: UniverseId::nil(),
                created_at_ns: request.forked_at_ns,
                manifest_hash: manifest.manifest_hash,
                active_baseline,
                journal_head: head,
            },
        })
    }

    pub fn manifest(&self, world_id: WorldId) -> Result<ManifestResponse, NodeHarnessError> {
        Ok(self.facade.manifest(world_id)?)
    }

    pub fn state_get(
        &self,
        world_id: WorldId,
        workflow: &str,
        key: Option<Vec<u8>>,
        consistency: Option<&str>,
    ) -> Result<crate::control::StateGetResponse, NodeHarnessError> {
        Ok(self.facade.state_get(
            world_id,
            workflow,
            StateGetQuery {
                key_b64: key.map(|bytes| BASE64_STANDARD.encode(bytes)),
                consistency: consistency.map(str::to_owned),
            },
        )?)
    }

    pub fn state_list(
        &self,
        world_id: WorldId,
        workflow: &str,
        limit: u32,
        consistency: Option<&str>,
    ) -> Result<StateListResponse, NodeHarnessError> {
        Ok(self.facade.state_list(
            world_id,
            workflow,
            LimitQuery {
                limit,
                consistency: consistency.map(str::to_owned),
            },
        )?)
    }

    pub fn get_blob(&self, hash: Hash) -> Result<Vec<u8>, NodeHarnessError> {
        Ok(self.runtime.get_blob(UniverseId::nil(), hash)?)
    }

    pub fn runtime(
        &self,
        world_id: WorldId,
    ) -> Result<crate::control::HostedWorldRuntimeResponse, NodeHarnessError> {
        Ok(self.facade.runtime(world_id)?)
    }

    pub fn journal_head(&self, world_id: WorldId) -> Result<HeadInfoResponse, NodeHarnessError> {
        Ok(self.facade.journal_head(world_id)?)
    }

    pub fn journal_entries(
        &self,
        world_id: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<JournalEntriesResponse, NodeHarnessError> {
        Ok(self
            .facade
            .journal_entries(world_id, JournalQuery { from, limit })?)
    }

    pub fn trace_summary(
        &self,
        world_id: WorldId,
        recent_limit: u32,
    ) -> Result<serde_json::Value, NodeHarnessError> {
        Ok(self.facade.trace_summary(world_id, recent_limit)?)
    }

    fn world_summary(&self, world_id: WorldId) -> Result<NodeHarnessStep, NodeHarnessError> {
        Ok(NodeHarnessStep {
            runtime: self.runtime.runtime_info(UniverseId::nil(), world_id)?,
            active_baseline: self.runtime.active_baseline(UniverseId::nil(), world_id)?,
        })
    }
}
