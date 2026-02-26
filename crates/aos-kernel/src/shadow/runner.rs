use std::sync::Arc;

use aos_cbor::{Hash, to_canonical_cbor};
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_store::Store;

use crate::journal::mem::MemJournal;
use crate::world::{Kernel, KernelConfig};
use crate::{
    error::KernelError,
    shadow::{
        ModuleEffectAllowlist, PendingWorkflowReceipt, PredictedEffect, ShadowConfig,
        ShadowSummary, WorkflowInstancePreview,
    },
};
use base64::Engine as _;
use hex;
use serde_json::Value as JsonValue;

pub struct ShadowExecutor;

impl ShadowExecutor {
    pub fn run<S: Store + 'static>(
        store: Arc<S>,
        config: &ShadowConfig,
    ) -> Result<ShadowSummary, KernelError> {
        let patch_bytes = to_canonical_cbor(&config.patch)
            .map_err(|err| KernelError::Manifest(format!("encode patch: {err}")))?;
        let expected_hash = Hash::from_hex_str(&config.patch_hash)
            .map_err(|err| KernelError::Manifest(format!("invalid patch hash: {err}")))?;
        let actual_hash = Hash::of_bytes(&patch_bytes);
        if expected_hash != actual_hash {
            return Err(KernelError::ShadowPatchMismatch {
                expected: expected_hash.to_hex(),
                actual: actual_hash.to_hex(),
            });
        }

        let loaded = config.patch.to_loaded_manifest(store.as_ref())?;
        let module_effect_allowlists = loaded
            .modules
            .values()
            .filter_map(|module| {
                let workflow = module.abi.workflow.as_ref()?;
                let mut effects = workflow
                    .effects_emitted
                    .iter()
                    .map(|kind| kind.as_str().to_string())
                    .collect::<Vec<_>>();
                effects.sort();
                Some(ModuleEffectAllowlist {
                    module: module.name.clone(),
                    effects_emitted: effects,
                })
            })
            .collect::<Vec<_>>();
        let mut kernel = Kernel::from_loaded_manifest_with_config(
            store.clone(),
            loaded,
            Box::new(MemJournal::new()),
            KernelConfig {
                allow_placeholder_secrets: true,
                ..KernelConfig::default()
            },
        )?;

        if let Some(harness) = &config.harness {
            for (schema, bytes) in &harness.seed_events {
                kernel.submit_domain_event(schema.clone(), bytes.clone())?;
            }
        }

        let mut predicted_effects = Vec::new();
        let mut pending_workflow_receipts = Vec::new();

        loop {
            kernel.tick_until_idle()?;
            let intents = kernel.drain_effects()?;
            if intents.is_empty() {
                break;
            }

            for intent in intents {
                predicted_effects.push(PredictedEffect {
                    kind: intent.kind.as_str().to_string(),
                    cap: intent.cap_name.clone(),
                    intent_hash: hex::encode(intent.intent_hash),
                    params_json: params_to_json(&intent.params_cbor),
                });

                // Prefer real internal handling so shadow predictions stay faithful.
                if let Some(receipt) = kernel.handle_internal_intent(&intent)? {
                    kernel.handle_receipt(receipt)?;
                    continue;
                }

                let receipt = EffectReceipt {
                    intent_hash: intent.intent_hash,
                    adapter_id: "shadow.mock".into(),
                    status: ReceiptStatus::Ok,
                    payload_cbor: Vec::new(),
                    cost_cents: None,
                    signature: Vec::new(),
                };
                kernel.handle_receipt(receipt)?;
            }
        }

        let workflow_instances = kernel.workflow_instances_snapshot();
        for instance in &workflow_instances {
            for inflight in &instance.inflight_intents {
                pending_workflow_receipts.push(PendingWorkflowReceipt {
                    instance_id: instance.instance_id.clone(),
                    origin_module_id: inflight.origin_module_id.clone(),
                    origin_instance_key_b64: inflight
                        .origin_instance_key
                        .as_ref()
                        .map(|key| base64::prelude::BASE64_STANDARD.encode(key)),
                    intent_hash: hex::encode(inflight.intent_id),
                    effect_kind: inflight.effect_kind.clone(),
                    emitted_at_seq: inflight.emitted_at_seq,
                });
            }
        }

        let workflow_instances = workflow_instances
            .into_iter()
            .map(|instance| WorkflowInstancePreview {
                instance_id: instance.instance_id,
                status: match instance.status {
                    crate::snapshot::WorkflowStatusSnapshot::Running => "running".to_string(),
                    crate::snapshot::WorkflowStatusSnapshot::Waiting => "waiting".to_string(),
                    crate::snapshot::WorkflowStatusSnapshot::Completed => "completed".to_string(),
                    crate::snapshot::WorkflowStatusSnapshot::Failed => "failed".to_string(),
                },
                last_processed_event_seq: instance.last_processed_event_seq,
                module_version: instance.module_version,
                inflight_intents: instance.inflight_intents.len(),
            })
            .collect::<Vec<_>>();

        let manifest_bytes = to_canonical_cbor(&config.patch.manifest)
            .map_err(|err| KernelError::Manifest(format!("encode manifest: {err}")))?;
        let manifest_hash = Hash::of_bytes(&manifest_bytes).to_hex();

        Ok(ShadowSummary {
            manifest_hash,
            predicted_effects,
            pending_workflow_receipts,
            workflow_instances,
            module_effect_allowlists,
            ledger_deltas: Vec::new(),
        })
    }
}

fn params_to_json(params_cbor: &[u8]) -> Option<JsonValue> {
    let cbor_value: serde_cbor::Value = serde_cbor::from_slice(params_cbor).ok()?;
    serde_json::to_value(&cbor_value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governance::ManifestPatch;
    use aos_air_types::{
        AirNode, CapEnforcer, CapType, DefCap, HashRef, Manifest, NamedRef, SecretDecl,
        SecretEntry, TypeExpr, TypeRecord,
    };
    use aos_store::MemStore;

    fn empty_manifest() -> Manifest {
        Manifest {
            air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
            schemas: vec![],
            modules: vec![],
            effects: vec![],
            caps: vec![],
            policies: vec![],
            secrets: vec![],
            defaults: None,
            module_bindings: Default::default(),
            routing: None,
        }
    }

    fn hash_of_patch(patch: &ManifestPatch) -> String {
        let bytes = to_canonical_cbor(patch).expect("canonical patch bytes");
        Hash::of_bytes(&bytes).to_hex()
    }

    fn hash_of_manifest(patch: &ManifestPatch) -> String {
        let bytes = to_canonical_cbor(&patch.manifest).expect("canonical manifest bytes");
        Hash::of_bytes(&bytes).to_hex()
    }

    #[test]
    fn shadow_executor_rejects_hash_mismatch() {
        let store = Arc::new(MemStore::new());
        let patch = ManifestPatch {
            manifest: empty_manifest(),
            nodes: vec![],
        };
        let config = ShadowConfig {
            proposal_id: 7,
            patch,
            patch_hash: "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                .into(),
            harness: None,
        };

        let result = ShadowExecutor::run(store, &config);
        assert!(matches!(
            result,
            Err(KernelError::ShadowPatchMismatch { .. })
        ));
    }

    #[test]
    fn shadow_executor_sets_manifest_hash_on_summary() {
        let store = Arc::new(MemStore::new());
        let cap = DefCap {
            name: "cap@1".into(),
            cap_type: CapType::new("custom"),
            schema: TypeExpr::Record(TypeRecord {
                record: Default::default(),
            }),
            enforcer: CapEnforcer {
                module: "sys/CapAllowAll@1".into(),
            },
        };
        let cap_hash = HashRef::new(
            Hash::of_cbor(&AirNode::Defcap(cap.clone()))
                .unwrap()
                .to_hex(),
        )
        .unwrap();
        let patch = ManifestPatch {
            manifest: Manifest {
                caps: vec![NamedRef {
                    name: cap.name.clone(),
                    hash: cap_hash,
                }],
                ..empty_manifest()
            },
            nodes: vec![AirNode::Defcap(cap)],
        };
        let patch_hash = hash_of_patch(&patch);
        let manifest_hash = hash_of_manifest(&patch);
        let summary = ShadowExecutor::run(
            store,
            &ShadowConfig {
                proposal_id: 1,
                patch,
                patch_hash: patch_hash.clone(),
                harness: None,
            },
        )
        .expect("shadow run");

        assert_eq!(summary.manifest_hash, manifest_hash);
    }

    #[test]
    fn shadow_executor_uses_placeholder_when_secrets_present() {
        let store = Arc::new(MemStore::new());
        let patch = ManifestPatch {
            manifest: Manifest {
                secrets: vec![SecretEntry::Decl(SecretDecl {
                    alias: "payments/stripe".into(),
                    version: 1,
                    binding_id: "stripe:prod".into(),
                    expected_digest: None,
                    policy: None,
                })],
                ..empty_manifest()
            },
            nodes: vec![],
        };
        let patch_hash = hash_of_patch(&patch);
        let manifest_hash = hash_of_manifest(&patch);

        let summary = ShadowExecutor::run(
            store,
            &ShadowConfig {
                proposal_id: 1,
                patch,
                patch_hash: patch_hash.clone(),
                harness: None,
            },
        )
        .expect("shadow should allow placeholder secrets");

        assert_eq!(summary.manifest_hash, manifest_hash);
    }
}
