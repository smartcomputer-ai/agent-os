use crate::contracts::{
    PlannedToolCall, ToolBatchId, ToolBatchPlan, ToolCallLlmResult, ToolCallObserved,
};
use alloc::string::String;
use alloc::vec::Vec;
use aos_wasm_sdk::PendingEffectLookupError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionRuntimeLimits {
    pub max_pending_effects: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunToolBatch<'a> {
    pub intent_id: &'a str,
    pub params_hash: Option<&'a String>,
    pub calls: &'a [ToolCallObserved],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedToolBatch {
    pub tool_batch_id: ToolBatchId,
    pub plan: ToolBatchPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedToolBatch {
    pub tool_batch_id: ToolBatchId,
    pub accepted_calls: Vec<PlannedToolCall>,
    pub ordered_results: Vec<ToolCallLlmResult>,
    pub results_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunToolBatchResult {
    pub started: StartedToolBatch,
    pub completion: Option<CompletedToolBatch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolBatchReceiptMatch {
    Unmatched,
    Matched {
        completion: Option<CompletedToolBatch>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionReduceError {
    InvalidLifecycleTransition,
    HostCommandRejected,
    ToolBatchAlreadyActive,
    MissingProvider,
    MissingModel,
    UnknownProvider,
    UnknownModel,
    RunAlreadyActive,
    RunNotActive,
    EmptyMessageRefs,
    TooManyPendingEffects,
    InvalidHashRef,
    ToolProfileUnknown,
    UnknownToolOverride,
    InvalidToolRegistry,
    AmbiguousPendingToolEffect,
}

impl SessionReduceError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidLifecycleTransition => "invalid lifecycle transition",
            Self::HostCommandRejected => "host command rejected",
            Self::ToolBatchAlreadyActive => "tool batch already active",
            Self::MissingProvider => "run config provider missing",
            Self::MissingModel => "run config model missing",
            Self::UnknownProvider => "run config provider unknown",
            Self::UnknownModel => "run config model unknown",
            Self::RunAlreadyActive => "run already active",
            Self::RunNotActive => "run not active",
            Self::EmptyMessageRefs => "llm message_refs must not be empty",
            Self::TooManyPendingEffects => "too many pending effects",
            Self::InvalidHashRef => "invalid hash ref",
            Self::ToolProfileUnknown => "tool profile unknown",
            Self::UnknownToolOverride => "unknown tool override",
            Self::InvalidToolRegistry => "invalid tool registry",
            Self::AmbiguousPendingToolEffect => "ambiguous pending tool effect",
        }
    }
}

impl core::fmt::Display for SessionReduceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl core::error::Error for SessionReduceError {}

pub(super) fn pending_effect_lookup_err_to_session_err(
    _err: PendingEffectLookupError,
) -> SessionReduceError {
    SessionReduceError::AmbiguousPendingToolEffect
}
