use std::collections::HashMap;

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
}

impl GovernanceManager {
    pub fn new() -> Self {
        Self {
            proposals: HashMap::new(),
        }
    }

    pub fn apply_record(&mut self, record: &GovernanceRecord) {
        match record {
            GovernanceRecord::ProposalSubmitted(submitted) => {
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
}
