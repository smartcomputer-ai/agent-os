use std::collections::HashMap;
use std::sync::Arc;

use aos_air_types::{AirNode, DefCap, DefModule, DefPlan, DefPolicy, Name};
use aos_store::Store;

use crate::journal::mem::MemJournal;
use crate::manifest::LoadedManifest;
use crate::world::Kernel;
use crate::{error::KernelError, shadow::{ShadowConfig, ShadowSummary}};
use hex;

pub struct ShadowExecutor;

impl ShadowExecutor {
    pub fn run<S: Store + 'static>(
        store: Arc<S>,
        config: &ShadowConfig,
    ) -> Result<ShadowSummary, KernelError> {
        let loaded = loaded_manifest_from_patch(&config.patch);
        let mut kernel = Kernel::from_loaded_manifest(
            store.clone(),
            loaded,
            Box::new(MemJournal::new()),
        )?;

        if let Some(harness) = &config.harness {
            for (schema, bytes) in &harness.seed_events {
                kernel.submit_domain_event(schema.clone(), bytes.clone());
            }
        }

        kernel.tick_until_idle()?;

        let intents = kernel.drain_effects();
        let predicted_effects = intents
            .into_iter()
            .map(|intent| format!("{}:{}", intent.kind.as_str(), hex::encode(intent.intent_hash)))
            .collect();
        let pending_receipts = kernel
            .pending_plan_receipts()
            .into_iter()
            .map(|(plan_id, hash)| format!("{}:{}", plan_id, hex::encode(hash)))
            .collect();
        let raised_events = Vec::new();

        Ok(ShadowSummary {
            predicted_effects,
            pending_receipts,
            raised_events,
        })
    }
}

fn loaded_manifest_from_patch(patch: &crate::governance::ManifestPatch) -> LoadedManifest {
    let mut modules: HashMap<Name, DefModule> = HashMap::new();
    let mut plans: HashMap<Name, DefPlan> = HashMap::new();
    let mut caps: HashMap<Name, DefCap> = HashMap::new();
    let mut policies: HashMap<Name, DefPolicy> = HashMap::new();
    for node in &patch.nodes {
        match node {
            AirNode::Defmodule(m) => {
                modules.insert(m.name.clone(), m.clone());
            }
            AirNode::Defplan(p) => {
                plans.insert(p.name.clone(), p.clone());
            }
            AirNode::Defcap(c) => {
                caps.insert(c.name.clone(), c.clone());
            }
            AirNode::Defpolicy(p) => {
                policies.insert(p.name.clone(), p.clone());
            }
            AirNode::Defschema(_) => {}
            AirNode::Manifest(_) => {}
        }
    }
    LoadedManifest {
        manifest: patch.manifest.clone(),
        modules,
        plans,
        caps,
        policies,
    }
}
