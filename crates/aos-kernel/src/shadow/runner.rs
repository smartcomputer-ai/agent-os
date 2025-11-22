use std::sync::Arc;

use aos_cbor::{Hash, to_canonical_cbor};
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_store::Store;

use crate::journal::mem::MemJournal;
use crate::world::{Kernel, KernelConfig};
use crate::{
    error::KernelError,
    shadow::{PendingPlanReceipt, PlanResultPreview, PredictedEffect, ShadowConfig, ShadowSummary},
};
use hex;

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

        let loaded = config.patch.to_loaded_manifest();
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
                kernel.submit_domain_event(schema.clone(), bytes.clone());
            }
        }

        let mut predicted_effects = Vec::new();
        let mut pending_receipts = Vec::new();

        loop {
            kernel.tick_until_idle()?;
            let intents = kernel.drain_effects();
            if intents.is_empty() {
                break;
            }

            for intent in intents {
                predicted_effects.push(PredictedEffect {
                    kind: intent.kind.as_str().to_string(),
                    cap: intent.cap_name.clone(),
                    intent_hash: hex::encode(intent.intent_hash),
                });

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

        for (plan_id, hash) in kernel.pending_plan_receipts() {
            pending_receipts.push(PendingPlanReceipt {
                plan_id,
                plan: kernel
                    .plan_name_for_instance(plan_id)
                    .map(ToOwned::to_owned),
                intent_hash: hex::encode(hash),
            });
        }

        let plan_results = kernel
            .recent_plan_results()
            .into_iter()
            .map(|result| PlanResultPreview {
                plan: result.plan_name,
                plan_id: result.plan_id,
                output_schema: result.output_schema,
            })
            .collect::<Vec<_>>();

        Ok(ShadowSummary {
            manifest_hash: config.patch_hash.clone(),
            predicted_effects,
            pending_receipts,
            plan_results,
            ledger_deltas: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governance::ManifestPatch;
    use aos_air_types::{HashRef, Manifest, NamedRef, SecretDecl};
    use aos_store::MemStore;

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

    fn hash_of_patch(patch: &ManifestPatch) -> String {
        let bytes = to_canonical_cbor(patch).expect("canonical patch bytes");
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
        let patch = ManifestPatch {
            manifest: Manifest {
                caps: vec![NamedRef {
                    name: "cap@1".into(),
                    hash: HashRef::new(
                        "sha256:0000000000000000000000000000000000000000000000000000000000000001",
                    )
                    .unwrap(),
                }],
                ..empty_manifest()
            },
            nodes: vec![],
        };
        let patch_hash = hash_of_patch(&patch);
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

        assert_eq!(summary.manifest_hash, patch_hash);
    }

    #[test]
    fn shadow_executor_uses_placeholder_when_secrets_present() {
        let store = Arc::new(MemStore::new());
        let patch = ManifestPatch {
            manifest: Manifest {
                secrets: vec![SecretDecl {
                    alias: "payments/stripe".into(),
                    version: 1,
                    binding_id: "stripe:prod".into(),
                    expected_digest: None,
                    policy: None,
                }],
                ..empty_manifest()
            },
            nodes: vec![],
        };
        let patch_hash = hash_of_patch(&patch);

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

        assert_eq!(summary.manifest_hash, patch_hash);
    }
}
