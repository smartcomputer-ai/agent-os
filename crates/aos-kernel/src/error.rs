use thiserror::Error;

#[derive(Debug, Error)]
pub enum KernelError {
    #[error("store error: {0}")]
    Store(#[from] crate::StoreError),
    #[error("wasm runtime error: {0}")]
    Wasm(#[from] anyhow::Error),
    #[error("manifest loader error: {0}")]
    Manifest(String),
    #[error("missing workflow '{0}'")]
    WorkflowNotFound(String),
    #[error("missing pure module '{0}'")]
    PureNotFound(String),
    #[error("invalid workflow output: {0}")]
    WorkflowOutput(String),
    #[error("effect manager error: {0}")]
    EffectManager(String),
    #[error("unknown effect receipt for {0}")]
    UnknownReceipt(String),
    #[error("failed to decode receipt payload: {0}")]
    ReceiptDecode(String),
    #[error("unsupported workflow receipt kind '{0}'")]
    UnsupportedWorkflowReceipt(String),
    #[error("unsupported effect kind '{0}'")]
    UnsupportedEffect(String),
    #[error("invalid idempotency key: {0}")]
    IdempotencyKeyInvalid(String),
    #[error("journal error: {0}")]
    Journal(String),
    #[error("snapshot unavailable: {0}")]
    SnapshotUnavailable(String),
    #[error("snapshot decode error: {0}")]
    SnapshotDecode(String),
    #[error("proposal {0} not found")]
    ProposalNotFound(u64),
    #[error("proposal {proposal_id} is in state {state:?}, expected {required}")]
    ProposalStateInvalid {
        proposal_id: u64,
        state: crate::governance::ProposalState,
        required: &'static str,
    },
    #[error("proposal {0} already applied")]
    ProposalAlreadyApplied(u64),
    #[error(
        "manifest apply blocked: in-flight runtime state exists (plans={plan_instances}, waiting_events={waiting_events}, pending_plan_receipts={pending_plan_receipts}, pending_workflow_receipts={pending_workflow_receipts}, queued_effects={queued_effects}, workflow_queue_pending={workflow_queue_pending})"
    )]
    ManifestApplyBlockedInFlight {
        plan_instances: usize,
        waiting_events: usize,
        pending_plan_receipts: usize,
        pending_workflow_receipts: usize,
        queued_effects: usize,
        workflow_queue_pending: bool,
    },
    #[error("shadow patch hash mismatch: expected {expected}, got {actual}")]
    ShadowPatchMismatch { expected: String, actual: String },
    #[error("secret resolver missing for manifest containing secrets")]
    SecretResolverMissing,
    #[error("secret resolve error: {0}")]
    SecretResolver(String),
    #[error("secret validation failed: {0}")]
    SecretResolution(String),
    #[error("manifest validation error: {0}")]
    ManifestValidation(String),
    #[error("query error: {0}")]
    Query(String),
    #[error("entropy error: {0}")]
    Entropy(String),
}

impl KernelError {
    /// Stable error taxonomy code for diagnostics and tooling.
    pub fn code(&self) -> &str {
        match self {
            KernelError::Store(_) => "store.error",
            KernelError::Wasm(_) => "wasm.error",
            KernelError::Manifest(_) => "manifest.error",
            KernelError::WorkflowNotFound(_) => "workflow.missing",
            KernelError::PureNotFound(_) => "pure.missing",
            KernelError::WorkflowOutput(_) => "workflow.output_invalid",
            KernelError::EffectManager(_) => "effect.manager",
            KernelError::UnknownReceipt(_) => "receipt.unknown",
            KernelError::ReceiptDecode(_) => "receipt.decode",
            KernelError::UnsupportedWorkflowReceipt(_) => "receipt.workflow_unsupported",
            KernelError::UnsupportedEffect(_) => "effect.op_unsupported",
            KernelError::IdempotencyKeyInvalid(_) => "idempotency.invalid",
            KernelError::Journal(_) => "journal.error",
            KernelError::SnapshotUnavailable(_) => "snapshot.unavailable",
            KernelError::SnapshotDecode(_) => "snapshot.decode",
            KernelError::ProposalNotFound(_) => "governance.proposal_missing",
            KernelError::ProposalStateInvalid { .. } => "governance.proposal_state",
            KernelError::ProposalAlreadyApplied(_) => "governance.proposal_applied",
            KernelError::ManifestApplyBlockedInFlight { .. } => "governance.apply_inflight",
            KernelError::ShadowPatchMismatch { .. } => "governance.shadow_mismatch",
            KernelError::SecretResolverMissing => "secret.resolver_missing",
            KernelError::SecretResolver(_) => "secret.resolver_error",
            KernelError::SecretResolution(_) => "secret.resolve_error",
            KernelError::ManifestValidation(_) => "manifest.validation",
            KernelError::Query(_) => "query.error",
            KernelError::Entropy(_) => "entropy.error",
        }
    }
}

impl From<crate::journal::JournalError> for KernelError {
    fn from(err: crate::journal::JournalError) -> Self {
        KernelError::Journal(err.to_string())
    }
}

impl From<crate::secret::SecretResolverError> for KernelError {
    fn from(err: crate::secret::SecretResolverError) -> Self {
        KernelError::SecretResolver(err.to_string())
    }
}
