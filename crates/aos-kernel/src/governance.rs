use std::collections::HashMap;

use crate::Store;
use crate::error::KernelError;
use crate::manifest::LoadedManifest;
use aos_air_types::{
    AirNode, DefModule, DefOp, DefSchema, DefSecret, Manifest, Name, builtins,
    catalog::EffectCatalog,
};
use aos_cbor::Hash;
use serde::{Deserialize, Serialize};

use crate::journal::{
    AppliedRecord, ApprovalDecisionRecord, ApprovedRecord, GovernanceRecord, ProposedRecord,
    ShadowReportRecord,
};
use crate::shadow::ShadowSummary;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProposalState {
    Submitted,
    Shadowed,
    Approved,
    Rejected,
    Applied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub id: u64,
    pub description: Option<String>,
    pub patch_hash: String,
    pub state: ProposalState,
    pub shadow_summary: Option<ShadowSummary>,
    pub approver: Option<String>,
}

impl Proposal {
    fn new(record: &ProposedRecord) -> Self {
        Self {
            id: record.proposal_id,
            description: record.description.clone(),
            patch_hash: record.patch_hash.clone(),
            state: ProposalState::Submitted,
            shadow_summary: None,
            approver: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct GovernanceManager {
    proposals: HashMap<u64, Proposal>,
    next_id: u64,
}

impl GovernanceManager {
    pub fn new() -> Self {
        Self {
            proposals: HashMap::new(),
            next_id: 0,
        }
    }

    pub fn alloc_proposal_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn observe_proposal_id(&mut self, id: u64) {
        if id >= self.next_id {
            self.next_id = id + 1;
        }
    }

    pub fn apply_record(&mut self, record: &GovernanceRecord) {
        match record {
            GovernanceRecord::Proposed(submitted) => {
                self.observe_proposal_id(submitted.proposal_id);
                self.proposals
                    .entry(submitted.proposal_id)
                    .or_insert_with(|| Proposal::new(submitted));
            }
            GovernanceRecord::ShadowReport(shadow) => {
                if let Some(proposal) = self.proposals.get_mut(&shadow.proposal_id) {
                    proposal.state = ProposalState::Shadowed;
                    proposal.shadow_summary = Some(ShadowSummary {
                        manifest_hash: shadow.manifest_hash.clone(),
                        predicted_effects: shadow.effects_predicted.clone(),
                        pending_workflow_receipts: shadow.pending_workflow_receipts.clone(),
                        workflow_instances: shadow.workflow_instances.clone(),
                        module_effect_allowlists: shadow.module_effect_allowlists.clone(),
                    });
                }
            }
            GovernanceRecord::Approved(approved) => {
                if let Some(proposal) = self.proposals.get_mut(&approved.proposal_id) {
                    proposal.state = match approved.decision {
                        ApprovalDecisionRecord::Approve => ProposalState::Approved,
                        ApprovalDecisionRecord::Reject => ProposalState::Rejected,
                    };
                    proposal.approver = Some(approved.approver.clone());
                }
            }
            GovernanceRecord::Applied(applied) => {
                self.observe_proposal_id(applied.proposal_id);
                if let Some(proposal) = self.proposals.get_mut(&applied.proposal_id) {
                    proposal.state = ProposalState::Applied;
                } else {
                    self.proposals.insert(
                        applied.proposal_id,
                        Proposal {
                            id: applied.proposal_id,
                            description: None,
                            patch_hash: applied.patch_hash.clone(),
                            state: ProposalState::Applied,
                            shadow_summary: None,
                            approver: None,
                        },
                    );
                }
            }
        }
    }

    pub fn proposals(&self) -> &HashMap<u64, Proposal> {
        &self.proposals
    }

    pub fn proposals_mut(&mut self) -> &mut HashMap<u64, Proposal> {
        &mut self.proposals
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestPatch {
    pub manifest: Manifest,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<AirNode>,
}

impl ManifestPatch {
    pub fn to_loaded_manifest<S: Store>(&self, store: &S) -> Result<LoadedManifest, KernelError> {
        let manifest = self.manifest.clone();
        let mut modules: HashMap<Name, DefModule> = HashMap::new();
        let mut ops: HashMap<Name, DefOp> = HashMap::new();
        let mut schemas: HashMap<Name, DefSchema> = HashMap::new();
        let mut secrets: Vec<DefSecret> = Vec::new();

        for node in &self.nodes {
            match node {
                AirNode::Defmodule(m) => {
                    modules.insert(m.name.clone(), m.clone());
                }
                AirNode::Defop(o) => {
                    ops.insert(o.name.clone(), o.clone());
                }
                AirNode::Defschema(s) => {
                    schemas.insert(s.name.clone(), s.clone());
                }
                AirNode::Defsecret(s) => {
                    parse_secret_name(&s.name)?;
                    secrets.push(s.clone());
                }
                AirNode::Manifest(_) => {}
            }
        }

        // Ensure built-ins referenced by the manifest are present in catalogs.
        for builtin in builtins::builtin_schemas() {
            if manifest
                .schemas
                .iter()
                .any(|nr| nr.name == builtin.schema.name)
            {
                schemas
                    .entry(builtin.schema.name.clone())
                    .or_insert(builtin.schema.clone());
            }
        }
        for builtin in builtins::builtin_ops() {
            if manifest.ops.iter().any(|nr| nr.name == builtin.op.name) {
                ops.entry(builtin.op.name.clone())
                    .or_insert(builtin.op.clone());
            }
        }
        for builtin in builtins::builtin_modules() {
            if manifest
                .modules
                .iter()
                .any(|nr| nr.name == builtin.module.name)
            {
                modules
                    .entry(builtin.module.name.clone())
                    .or_insert(builtin.module.clone());
            }
        }

        load_defs_from_manifest(
            store,
            &manifest.schemas,
            &mut schemas,
            "defschema",
            |node| {
                if let AirNode::Defschema(schema) = node {
                    Ok(schema)
                } else {
                    Err(KernelError::Manifest(
                        "manifest schema ref did not point to defschema".into(),
                    ))
                }
            },
        )?;
        load_defs_from_manifest(
            store,
            &manifest.modules,
            &mut modules,
            "defmodule",
            |node| {
                if let AirNode::Defmodule(module) = node {
                    Ok(module)
                } else {
                    Err(KernelError::Manifest(
                        "manifest module ref did not point to defmodule".into(),
                    ))
                }
            },
        )?;
        load_defs_from_manifest(store, &manifest.ops, &mut ops, "defop", |node| {
            if let AirNode::Defop(op) = node {
                Ok(op)
            } else {
                Err(KernelError::Manifest(
                    "manifest op ref did not point to defop".into(),
                ))
            }
        })?;
        for reference in &manifest.secrets {
            if secrets.iter().any(|secret| secret.name == reference.name) {
                continue;
            }
            let hash = parse_manifest_hash(reference.hash.as_str())?;
            let node: AirNode = store
                .get_node(hash)
                .map_err(|err| KernelError::Manifest(format!("load secret: {err}")))?;
            match node {
                AirNode::Defsecret(secret) => {
                    parse_secret_name(&secret.name)?;
                    secrets.push(secret);
                }
                _ => {
                    return Err(KernelError::Manifest(
                        "manifest secret ref did not point to defsecret".into(),
                    ));
                }
            }
        }

        if ops.is_empty() {
            for builtin in builtins::builtin_ops() {
                ops.insert(builtin.op.name.clone(), builtin.op.clone());
            }
        }
        let effect_catalog = EffectCatalog::from_defs(ops.values().cloned());
        Ok(LoadedManifest {
            manifest,
            secrets,
            modules,
            ops,
            schemas,
            effect_catalog,
        })
    }
}

fn parse_manifest_hash(value: &str) -> Result<Hash, KernelError> {
    let hash = Hash::from_hex_str(value)
        .map_err(|err| KernelError::Manifest(format!("invalid hash '{value}': {err}")))?;
    if hash.as_bytes().iter().all(|b| *b == 0) {
        return Err(KernelError::Manifest(format!(
            "missing manifest ref hash for '{value}'"
        )));
    }
    Ok(hash)
}

fn load_defs_from_manifest<T>(
    store: &impl Store,
    refs: &[aos_air_types::NamedRef],
    defs: &mut HashMap<Name, T>,
    label: &str,
    decode: impl FnOnce(AirNode) -> Result<T, KernelError> + Copy,
) -> Result<(), KernelError> {
    for reference in refs {
        if defs.contains_key(reference.name.as_str()) {
            continue;
        }
        let hash = parse_manifest_hash(reference.hash.as_str())?;
        let node: AirNode = store
            .get_node(hash)
            .map_err(|err| KernelError::Manifest(format!("load {label}: {err}")))?;
        let def = decode(node)?;
        defs.insert(reference.name.clone(), def);
    }
    Ok(())
}

fn parse_secret_name(name: &str) -> Result<(String, u64), KernelError> {
    let mut parts = name.rsplitn(2, '@');
    let version_raw = parts.next().ok_or_else(|| {
        KernelError::Manifest(format!(
            "invalid defsecret name '{name}': missing version segment"
        ))
    })?;
    let version_part = version_raw.parse::<u64>().map_err(|_| {
        KernelError::Manifest(format!(
            "invalid defsecret name '{name}': version must be a positive integer"
        ))
    })?;
    if version_part == 0 {
        return Err(KernelError::Manifest(format!(
            "invalid defsecret name '{name}': version must be >= 1"
        )));
    }
    let alias = parts
        .next()
        .filter(|alias| !alias.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            KernelError::Manifest(format!(
                "invalid defsecret name '{name}': missing alias prefix"
            ))
        })?;
    Ok((alias, version_part))
}
