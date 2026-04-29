use super::{
    ContextObservation, HostCommand, HostSessionStatus, RunCause, RunId, RunLifecycle,
    SessionConfig, SessionId, SessionLifecycle, SessionStatus, ToolOverrideScope, ToolSpec,
};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use aos_wasm_sdk::{AirSchema, EffectContinuationRef, EffectReceiptEnvelope};
use serde::{Deserialize, Serialize};

pub use aos_wasm_sdk::{EffectReceiptRejected, EffectStreamFrameEnvelope};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/SessionInputKind@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum SessionInputKind {
    RunRequested {
        #[aos(air_type = "hash")]
        input_ref: String,
        run_overrides: Option<SessionConfig>,
    },
    RunStartRequested {
        cause: RunCause,
        run_overrides: Option<SessionConfig>,
    },
    FollowUpInputAppended {
        #[aos(air_type = "hash")]
        input_ref: String,
        run_overrides: Option<SessionConfig>,
    },
    RunSteerRequested {
        #[aos(air_type = "hash")]
        instruction_ref: String,
    },
    RunInterruptRequested {
        #[aos(air_type = "hash")]
        reason_ref: Option<String>,
    },
    SessionOpened {
        config: Option<SessionConfig>,
    },
    SessionConfigUpdated {
        config: SessionConfig,
    },
    SessionPaused,
    SessionResumed,
    SessionArchived,
    SessionExpired,
    SessionClosed,
    HostCommandReceived(HostCommand),
    ToolRegistrySet {
        registry: BTreeMap<String, ToolSpec>,
        profiles: Option<BTreeMap<String, Vec<String>>>,
        default_profile: Option<String>,
    },
    ToolProfileSelected {
        profile_id: String,
    },
    ToolOverridesSet {
        scope: ToolOverrideScope,
        enable: Option<Vec<String>>,
        disable: Option<Vec<String>>,
        force: Option<Vec<String>>,
    },
    ContextObserved(ContextObservation),
    HostSessionUpdated {
        host_session_id: Option<String>,
        host_session_status: Option<HostSessionStatus>,
    },
    RunCompleted,
    RunFailed {
        code: String,
        detail: String,
    },
    RunCancelled {
        reason: Option<String>,
    },
    #[default]
    #[aos(schema_ref = SessionNoop)]
    Noop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/SessionInput@1")]
pub struct SessionInput {
    pub session_id: SessionId,
    #[aos(air_type = "time")]
    pub observed_at_ns: u64,
    pub input: SessionInputKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/SessionLifecycleChanged@1")]
pub struct SessionLifecycleChanged {
    pub session_id: SessionId,
    #[aos(air_type = "time")]
    pub observed_at_ns: u64,
    pub from: SessionLifecycle,
    pub to: SessionLifecycle,
    pub run_id: Option<RunId>,
    #[aos(air_type = "hash")]
    pub output_ref: Option<String>,
    pub in_flight_effects: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/SessionStatusChanged@1")]
pub struct SessionStatusChanged {
    pub session_id: SessionId,
    #[aos(air_type = "time")]
    pub observed_at_ns: u64,
    pub from: SessionStatus,
    pub to: SessionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/RunLifecycleChanged@1")]
pub struct RunLifecycleChanged {
    pub session_id: SessionId,
    pub run_id: RunId,
    #[aos(air_type = "time")]
    pub observed_at_ns: u64,
    pub from: RunLifecycle,
    pub to: RunLifecycle,
    pub cause: RunCause,
    #[aos(air_type = "hash")]
    pub output_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/SessionWorkflowEvent@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum SessionWorkflowEvent {
    Input(SessionInput),
    Receipt(EffectReceiptEnvelope),
    ReceiptRejected(EffectReceiptRejected),
    StreamFrame(EffectStreamFrameEnvelope),
    #[default]
    #[aos(schema_ref = SessionNoop)]
    Noop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/SessionNoop@1")]
pub struct SessionNoop;

impl SessionWorkflowEvent {
    pub fn continuation(&self) -> Option<EffectContinuationRef<'_>> {
        match self {
            Self::Receipt(value) => Some(value.into()),
            Self::ReceiptRejected(value) => Some(value.into()),
            Self::StreamFrame(value) => Some(value.into()),
            Self::Input(_) | Self::Noop => None,
        }
    }
}
