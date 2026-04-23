use super::{
    HostCommand, HostSessionStatus, RunId, SessionConfig, SessionId, SessionLifecycle,
    ToolOverrideScope, ToolSpec,
};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use aos_wasm_sdk::{AirSchema, EffectContinuationRef, EffectReceiptEnvelope};
use serde::{Deserialize, Serialize};

pub use aos_wasm_sdk::{EffectReceiptRejected, EffectStreamFrameEnvelope};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/SessionIngressKind@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum SessionIngressKind {
    RunRequested {
        #[aos(air_type = "hash")]
        input_ref: String,
        run_overrides: Option<SessionConfig>,
    },
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
    #[aos(schema_ref = "aos.agent/SessionNoop@1")]
    Noop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/SessionIngress@1")]
pub struct SessionIngress {
    pub session_id: SessionId,
    #[aos(air_type = "time")]
    pub observed_at_ns: u64,
    pub ingress: SessionIngressKind,
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
#[aos(schema = "aos.agent/SessionWorkflowEvent@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum SessionWorkflowEvent {
    Ingress(SessionIngress),
    Receipt(EffectReceiptEnvelope),
    ReceiptRejected(EffectReceiptRejected),
    StreamFrame(EffectStreamFrameEnvelope),
    #[default]
    #[aos(schema_ref = "aos.agent/SessionNoop@1")]
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
            Self::Ingress(_) | Self::Noop => None,
        }
    }
}
