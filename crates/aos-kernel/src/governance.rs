use std::collections::HashMap;

use crate::manifest::LoadedManifest;
use aos_air_types::{AirNode, DefCap, DefModule, DefPlan, DefPolicy, DefSchema, Manifest, Name};
use serde::{Deserialize, Serialize};

use crate::journal::{
    GovernanceRecord, ManifestAppliedRecord, ProposalApprovedRecord, ProposalSubmittedRecord,
    ShadowRunCompletedRecord,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProposalState {
    Submitted,
    Shadowed,
    Approved,
    Applied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub id: u64,
    pub description: Option<String>,
    pub patch_hash: String,
    pub state: ProposalState,
    pub shadow_summary: Option<Vec<u8>>,
    pub approver: Option<String>,
}

impl Proposal {
    fn new(record: &ProposalSubmittedRecord) -> Self {
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
            GovernanceRecord::ProposalSubmitted(submitted) => {
                self.observe_proposal_id(submitted.proposal_id);
                self.proposals
                    .entry(submitted.proposal_id)
                    .or_insert_with(|| Proposal::new(submitted));
            }
            GovernanceRecord::ShadowRunCompleted(shadow) => {
                if let Some(proposal) = self.proposals.get_mut(&shadow.proposal_id) {
                    proposal.state = ProposalState::Shadowed;
                    proposal.shadow_summary = Some(shadow.summary.clone());
                }
            }
            GovernanceRecord::ProposalApproved(approved) => {
                if let Some(proposal) = self.proposals.get_mut(&approved.proposal_id) {
                    proposal.state = ProposalState::Approved;
                    proposal.approver = Some(approved.approver.clone());
                }
            }
            GovernanceRecord::ManifestApplied(applied) => {
                self.observe_proposal_id(applied.proposal_id);
                if let Some(proposal) = self.proposals.get_mut(&applied.proposal_id) {
                    proposal.state = ProposalState::Applied;
                } else {
                    self.proposals.insert(
                        applied.proposal_id,
                        Proposal {
                            id: applied.proposal_id,
                            description: None,
                            patch_hash: applied.manifest_hash.clone(),
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
        let mut modules: HashMap<Name, DefModule> = HashMap::new();
        let mut plans: HashMap<Name, DefPlan> = HashMap::new();
        let mut caps: HashMap<Name, DefCap> = HashMap::new();
        let mut policies: HashMap<Name, DefPolicy> = HashMap::new();
        let mut schemas: HashMap<Name, DefSchema> = HashMap::new();
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
                AirNode::Defpolicy(p) => {
                    policies.insert(p.name.clone(), p.clone());
                }
                AirNode::Defschema(s) => {
                    schemas.insert(s.name.clone(), s.clone());
                }
                _ => {}
            }
        }
        LoadedManifest {
            manifest: self.manifest.clone(),
            modules,
            plans,
            caps,
            policies,
            schemas,
        }
    }
}
