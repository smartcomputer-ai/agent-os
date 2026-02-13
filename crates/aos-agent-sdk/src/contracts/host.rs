use super::RunId;
use alloc::string::String;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RunLease {
    pub lease_id: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub heartbeat_timeout_secs: u64,
}

impl RunLease {
    pub fn is_expired_at(&self, observed_time_ns: u64) -> bool {
        observed_time_ns >= self.expires_at
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HostCommand {
    pub command_id: String,
    pub target_run_id: Option<RunId>,
    pub expected_session_epoch: Option<u64>,
    pub issued_at: u64,
    pub command: HostCommandKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
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
    LeaseHeartbeat {
        lease_id: String,
        heartbeat_at: u64,
    },
    #[default]
    Noop,
}
