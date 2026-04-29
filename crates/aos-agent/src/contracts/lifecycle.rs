use aos_wasm_sdk::AirSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/SessionStatus@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum SessionStatus {
    #[default]
    Open,
    Paused,
    Archived,
    Expired,
    Closed,
}

impl SessionStatus {
    pub fn accepts_new_runs(self) -> bool {
        matches!(self, Self::Open)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/RunLifecycle@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum RunLifecycle {
    Queued,
    #[default]
    Running,
    WaitingInput,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl RunLifecycle {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/SessionLifecycle@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum SessionLifecycle {
    #[default]
    Idle,
    Running,
    WaitingInput,
    Paused,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}

impl SessionLifecycle {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}
