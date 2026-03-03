use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
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
