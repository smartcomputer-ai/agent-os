use thiserror::Error;

use crate::journal::CapDenyReason;

#[derive(Debug, Error)]
pub enum KernelError {
    #[error("store error: {0}")]
    Store(#[from] aos_store::StoreError),
    #[error("wasm runtime error: {0}")]
    Wasm(#[from] anyhow::Error),
    #[error("manifest loader error: {0}")]
    Manifest(String),
    #[error("missing reducer '{0}'")]
    ReducerNotFound(String),
    #[error("missing pure module '{0}'")]
    PureNotFound(String),
    #[error("invalid reducer output: {0}")]
    ReducerOutput(String),
    #[error("effect manager error: {0}")]
    EffectManager(String),
    #[error("unknown effect receipt for {0}")]
    UnknownReceipt(String),
    #[error("failed to decode receipt payload: {0}")]
    ReceiptDecode(String),
    #[error("unsupported reducer receipt kind '{0}'")]
    UnsupportedReducerReceipt(String),
    #[error("capability grant '{0}' not found")]
    CapabilityGrantNotFound(String),
    #[error("capability definition '{0}' not found")]
    CapabilityDefinitionNotFound(String),
    #[error("duplicate capability grant '{0}'")]
    DuplicateCapabilityGrant(String),
    #[error("capability params encoding error: {0}")]
    CapabilityEncoding(String),
    #[error(
        "effect '{effect_kind}' requires capability type '{expected}' but grant '{grant}' provides '{found}'"
    )]
    CapabilityTypeMismatch {
        grant: String,
        effect_kind: String,
        expected: String,
        found: String,
    },
    #[error("unsupported effect kind '{0}'")]
    UnsupportedEffectKind(String),
    #[error("capability binding missing for reducer '{reducer}' slot '{slot}'")]
    CapabilityBindingMissing { reducer: String, slot: String },
    #[error("capability grant '{cap}' referenced by plan '{plan}' is missing")]
    PlanCapabilityMissing { plan: String, cap: String },
    #[error("module '{module}' binding references missing capability '{cap}'")]
    ModuleCapabilityMissing { module: String, cap: String },
    #[error(
        "policy '{policy_name}' denied effect '{effect_kind}' from {origin} (rule_index={rule_index:?})"
    )]
    PolicyDenied {
        effect_kind: String,
        origin: String,
        policy_name: String,
        rule_index: Option<u32>,
    },
    #[error("capability grant '{grant}' params do not match schema for '{cap}': {reason}")]
    CapabilityParamInvalid {
        grant: String,
        cap: String,
        reason: String,
    },
    #[error("capability denied for '{cap}' on effect '{effect_kind}': {reason}")]
    CapabilityDenied {
        cap: String,
        effect_kind: String,
        reason: CapDenyReason,
    },
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
        "manifest apply blocked: in-flight runtime state exists (plans={plan_instances}, waiting_events={waiting_events}, pending_plan_receipts={pending_plan_receipts}, pending_reducer_receipts={pending_reducer_receipts}, queued_effects={queued_effects}, reducer_queue_pending={reducer_queue_pending})"
    )]
    ManifestApplyBlockedInFlight {
        plan_instances: usize,
        waiting_events: usize,
        pending_plan_receipts: usize,
        pending_reducer_receipts: usize,
        queued_effects: usize,
        reducer_queue_pending: bool,
    },
    #[error("shadow patch hash mismatch: expected {expected}, got {actual}")]
    ShadowPatchMismatch { expected: String, actual: String },
    #[error("secret resolver missing for manifest containing secrets")]
    SecretResolverMissing,
    #[error("secret resolve error: {0}")]
    SecretResolver(String),
    #[error("secret validation failed: {0}")]
    SecretResolution(String),
    #[error("secret policy denied for {alias}@{version}: {reason}")]
    SecretPolicyDenied {
        alias: String,
        version: u64,
        reason: String,
    },
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
            KernelError::ReducerNotFound(_) => "reducer.missing",
            KernelError::PureNotFound(_) => "pure.missing",
            KernelError::ReducerOutput(_) => "reducer.output_invalid",
            KernelError::EffectManager(_) => "effect.manager",
            KernelError::UnknownReceipt(_) => "receipt.unknown",
            KernelError::ReceiptDecode(_) => "receipt.decode",
            KernelError::UnsupportedReducerReceipt(_) => "receipt.reducer_unsupported",
            KernelError::CapabilityGrantNotFound(_) => "cap.grant_missing",
            KernelError::CapabilityDefinitionNotFound(_) => "cap.def_missing",
            KernelError::DuplicateCapabilityGrant(_) => "cap.grant_duplicate",
            KernelError::CapabilityEncoding(_) => "cap.params_encode",
            KernelError::CapabilityTypeMismatch { .. } => "cap.type_mismatch",
            KernelError::UnsupportedEffectKind(_) => "effect.kind_unsupported",
            KernelError::CapabilityBindingMissing { .. } => "cap.binding_missing",
            KernelError::PlanCapabilityMissing { .. } => "cap.plan_missing",
            KernelError::ModuleCapabilityMissing { .. } => "cap.module_missing",
            KernelError::PolicyDenied { .. } => "policy.denied",
            KernelError::CapabilityParamInvalid { .. } => "cap.params_invalid",
            KernelError::CapabilityDenied { reason, .. } => reason.code.as_str(),
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
            KernelError::SecretPolicyDenied { .. } => "secret.policy_denied",
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
