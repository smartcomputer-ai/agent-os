use std::collections::HashMap;

use crate::manifest::LoadedManifest;
use aos_air_types::{
    AirNode, DefCap, DefEffect, DefModule, DefPlan, DefPolicy, DefSchema, Manifest, Name,
    SecretDecl, SecretPolicy, catalog::EffectCatalog,
};
use serde::{Deserialize, Serialize};

use crate::journal::{
    AppliedRecord, ApprovedRecord, ApprovalDecisionRecord, GovernanceRecord, ProposedRecord,
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
                        pending_receipts: shadow.pending_receipts.clone(),
                        plan_results: shadow.plan_results.clone(),
                        ledger_deltas: shadow.ledger_deltas.clone(),
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
    pub fn to_loaded_manifest(&self) -> LoadedManifest {
        let mut manifest = self.manifest.clone();
        let mut modules: HashMap<Name, DefModule> = HashMap::new();
        let mut plans: HashMap<Name, DefPlan> = HashMap::new();
        let mut effects: HashMap<Name, DefEffect> = HashMap::new();
        let mut caps: HashMap<Name, DefCap> = HashMap::new();
        let mut policies: HashMap<Name, DefPolicy> = HashMap::new();
        let mut schemas: HashMap<Name, DefSchema> = HashMap::new();
        let mut secrets: Vec<SecretDecl> = Vec::new();
        for node in &self.nodes {
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
                AirNode::Defeffect(e) => {
                    effects.insert(e.name.clone(), e.clone());
                }
                AirNode::Defpolicy(p) => {
                    policies.insert(p.name.clone(), p.clone());
                }
                AirNode::Defschema(s) => {
                    schemas.insert(s.name.clone(), s.clone());
                }
                AirNode::Defsecret(s) => {
                    let (alias, version) = parse_secret_name(&s.name);
                    secrets.push(SecretDecl {
                        alias,
                        version,
                        binding_id: s.binding_id.clone(),
                        expected_digest: s.expected_digest.clone(),
                        policy: Some(SecretPolicy {
                            allowed_caps: s.allowed_caps.clone(),
                            allowed_plans: s.allowed_plans.clone(),
                        })
                        .filter(|p| !p.allowed_caps.is_empty() || !p.allowed_plans.is_empty()),
                    });
                }
                _ => {}
            }
        }
        // Ensure built-in schemas/effects are present so shadow validation has full catalogs.
        for builtin in aos_air_types::builtins::builtin_schemas() {
            schemas.entry(builtin.schema.name.clone()).or_insert(builtin.schema.clone());
            if !manifest.schemas.iter().any(|nr| nr.name == builtin.schema.name) {
                manifest.schemas.push(aos_air_types::NamedRef {
                    name: builtin.schema.name.clone(),
                    hash: builtin.hash_ref.clone(),
                });
            }
        }
        if effects.is_empty() {
            for builtin in aos_air_types::builtins::builtin_effects() {
                effects.insert(builtin.effect.name.clone(), builtin.effect.clone());
            }
        }
        for builtin in aos_air_types::builtins::builtin_effects() {
            if !manifest.effects.iter().any(|nr| nr.name == builtin.effect.name) {
                manifest.effects.push(aos_air_types::NamedRef {
                    name: builtin.effect.name.clone(),
                    hash: builtin.hash_ref.clone(),
                });
            }
        }
        let effect_catalog = EffectCatalog::from_defs(effects.values().cloned());
        LoadedManifest {
            manifest,
            secrets,
            modules,
            plans,
            effects,
            caps,
            policies,
            schemas,
            effect_catalog,
        }
    }
}

fn parse_secret_name(name: &str) -> (String, u64) {
    let mut parts = name.rsplitn(2, '@');
    let version_part = parts
        .next()
        .and_then(|p| p.parse::<u64>().ok())
        .filter(|v| *v >= 1)
        .expect("defsecret name must end with @<version>=1");
    let alias = parts
        .next()
        .map(str::to_string)
        .expect("defsecret name must include alias");
    (alias, version_part)
}
