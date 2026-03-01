use alloc::string::String;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HostCommand {
    pub command_id: String,
    pub issued_at: u64,
    pub command: HostCommandKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "$tag", content = "$value")]
pub enum HostCommandKind {
    Steer {
        text: String,
    },
    FollowUp {
        text: String,
    },
    Pause,
    Resume,
    Cancel {
        reason: Option<String>,
    },
    #[default]
    Noop,
}
