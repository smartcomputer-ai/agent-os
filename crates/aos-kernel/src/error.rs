use thiserror::Error;

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
    #[error("policy denied effect '{effect_kind}' from {origin}")]
    PolicyDenied { effect_kind: String, origin: String },
    #[error("capability grant '{grant}' params do not match schema for '{cap}': {reason}")]
    CapabilityParamInvalid {
        grant: String,
        cap: String,
        reason: String,
    },
    #[error("plan '{plan}' invariant #{index} failed")]
    PlanInvariantFailed { plan: String, index: usize },
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
    #[error("shadow patch hash mismatch: expected {expected}, got {actual}")]
    ShadowPatchMismatch { expected: String, actual: String },
    #[error("secret resolver missing for manifest containing secrets")]
    SecretResolverMissing,
    #[error("secret resolve error: {0}")]
    SecretResolver(String),
    #[error("secret validation failed: {0}")]
    SecretResolution(String),
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
