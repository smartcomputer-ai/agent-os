use serde::{Deserialize, Serialize};

/// Canonical failure code vocabulary for MVP P2.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    PolicyDenied,
    CapabilityDenied,
    ValidationError,
    AdapterTimeout,
    AdapterError,
    ProviderRetryable,
    ProviderTerminal,
    ToolNotFound,
    ToolInvalidArgs,
    InvariantViolation,
    UnknownFailure,
}

impl FailureCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PolicyDenied => "policy_denied",
            Self::CapabilityDenied => "capability_denied",
            Self::ValidationError => "validation_error",
            Self::AdapterTimeout => "adapter_timeout",
            Self::AdapterError => "adapter_error",
            Self::ProviderRetryable => "provider_retryable",
            Self::ProviderTerminal => "provider_terminal",
            Self::ToolNotFound => "tool_not_found",
            Self::ToolInvalidArgs => "tool_invalid_args",
            Self::InvariantViolation => "invariant_violation",
            Self::UnknownFailure => "unknown_failure",
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        match code {
            "policy_denied" => Some(Self::PolicyDenied),
            "capability_denied" => Some(Self::CapabilityDenied),
            "validation_error" => Some(Self::ValidationError),
            "adapter_timeout" => Some(Self::AdapterTimeout),
            "adapter_error" => Some(Self::AdapterError),
            "provider_retryable" => Some(Self::ProviderRetryable),
            "provider_terminal" => Some(Self::ProviderTerminal),
            "tool_not_found" => Some(Self::ToolNotFound),
            "tool_invalid_args" => Some(Self::ToolInvalidArgs),
            "invariant_violation" => Some(Self::InvariantViolation),
            "unknown_failure" => Some(Self::UnknownFailure),
            _ => None,
        }
    }
}

