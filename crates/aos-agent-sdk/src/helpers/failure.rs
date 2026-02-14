use crate::contracts::{FailureCode, SessionEventKind, ToolCallStatus};
use alloc::string::String;

/// Retry owner for a given failure path. The contract is single-owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryOwner {
    Adapter,
    Plan,
    Reducer,
}

/// Map canonical failure code to the single owning retry layer.
pub const fn retry_owner_for_failure(code: FailureCode) -> RetryOwner {
    match code {
        FailureCode::AdapterTimeout | FailureCode::AdapterError => RetryOwner::Adapter,
        FailureCode::ProviderRetryable => RetryOwner::Plan,
        FailureCode::PolicyDenied
        | FailureCode::CapabilityDenied
        | FailureCode::ValidationError
        | FailureCode::ProviderTerminal
        | FailureCode::ToolNotFound
        | FailureCode::ToolInvalidArgs
        | FailureCode::InvariantViolation
        | FailureCode::UnknownFailure => RetryOwner::Reducer,
    }
}

/// Deterministic helper for `RunFailed` event payloads using canonical codes.
pub fn run_failed_kind(code: FailureCode, detail: impl Into<String>) -> SessionEventKind {
    SessionEventKind::RunFailed {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_code_round_trip_strings() {
        let all = [
            FailureCode::PolicyDenied,
            FailureCode::CapabilityDenied,
            FailureCode::ValidationError,
            FailureCode::AdapterTimeout,
            FailureCode::AdapterError,
            FailureCode::ProviderRetryable,
            FailureCode::ProviderTerminal,
            FailureCode::ToolNotFound,
            FailureCode::ToolInvalidArgs,
            FailureCode::InvariantViolation,
            FailureCode::UnknownFailure,
        ];
        for code in all {
            let text = code.as_str();
            assert_eq!(FailureCode::parse(text), Some(code));
        }
        assert_eq!(FailureCode::parse("not_a_code"), None);
    }

    #[test]
    fn retry_owner_is_single_and_deterministic() {
        assert_eq!(
            retry_owner_for_failure(FailureCode::AdapterTimeout),
            RetryOwner::Adapter
        );
        assert_eq!(
            retry_owner_for_failure(FailureCode::AdapterError),
            RetryOwner::Adapter
        );
        assert_eq!(
            retry_owner_for_failure(FailureCode::ProviderRetryable),
            RetryOwner::Plan
        );
        assert_eq!(
            retry_owner_for_failure(FailureCode::ProviderTerminal),
            RetryOwner::Reducer
        );
        assert_eq!(
            retry_owner_for_failure(FailureCode::PolicyDenied),
            RetryOwner::Reducer
        );
    }

    #[test]
    fn run_failed_kind_uses_canonical_code() {
        let event = run_failed_kind(FailureCode::ValidationError, "invalid request");
        match event {
            SessionEventKind::RunFailed { code, detail } => {
                assert_eq!(code, "validation_error");
                assert_eq!(detail, "invalid request");
            }
            _ => panic!("expected RunFailed"),
        }
    }

    #[test]
    fn tool_call_failed_status_uses_canonical_code() {
        let status = tool_call_failed_status(FailureCode::ToolInvalidArgs, "missing field");
        match status {
            ToolCallStatus::Failed { code, detail } => {
                assert_eq!(code, "tool_invalid_args");
                assert_eq!(detail, "missing field");
            }
            _ => panic!("expected ToolCallStatus::Failed"),
        }
    }
}
