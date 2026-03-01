use crate::contracts::{FailureCode, SessionIngressKind, ToolCallStatus};
use alloc::string::String;

/// Retry owner for a given failure path. The contract is single-owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryOwner {
    Adapter,
    Workflow,
}

/// Map canonical failure code to the single owning retry layer.
pub const fn retry_owner_for_failure(code: FailureCode) -> RetryOwner {
    match code {
        FailureCode::AdapterTimeout
        | FailureCode::AdapterError
        | FailureCode::ProviderRetryable => RetryOwner::Adapter,
        FailureCode::PolicyDenied
        | FailureCode::CapabilityDenied
        | FailureCode::ValidationError
        | FailureCode::ProviderTerminal
        | FailureCode::ToolNotFound
        | FailureCode::ToolInvalidArgs
        | FailureCode::InvariantViolation
        | FailureCode::UnknownFailure => RetryOwner::Workflow,
    }
}

/// Deterministic helper for `RunFailed` ingress payloads using canonical codes.
pub fn run_failed_ingress(code: FailureCode, detail: impl Into<String>) -> SessionIngressKind {
    SessionIngressKind::RunFailed {
        code: code.as_str().into(),
        detail: detail.into(),
    }
}

/// Deterministic helper for tool-call failed status payloads using canonical codes.
pub fn tool_call_failed_status(code: FailureCode, detail: impl Into<String>) -> ToolCallStatus {
    ToolCallStatus::Failed {
        code: code.as_str().into(),
        detail: detail.into(),
    }
}
