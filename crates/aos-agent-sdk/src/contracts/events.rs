use super::{
    HostCommand, RunId, RunLease, SessionConfig, SessionId, SessionLifecycle, StepId, ToolBatchId,
    ToolCallStatus, TurnId, WorkspaceApplyMode, WorkspaceBinding, WorkspaceSnapshot,
};
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod serde_bytes_opt {
    use super::*;
    use serde_bytes::{ByteBuf, Bytes};

    pub fn serialize<S>(value: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(bytes) => serializer.serialize_some(Bytes::new(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<ByteBuf>::deserialize(deserializer).map(|opt| opt.map(|buf| buf.into_vec()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "$tag", content = "$value")]
pub enum SessionEventKind {
    RunRequested {
        input_ref: String,
        run_overrides: Option<SessionConfig>,
    },
    RunStarted,
    HostCommandReceived(HostCommand),
    HostCommandApplied {
        command_id: String,
    },
    LifecycleChanged(SessionLifecycle),
    StepBoundary,
    ToolBatchStarted {
        tool_batch_id: ToolBatchId,
        expected_call_ids: Vec<String>,
    },
    ToolCallSettled {
        tool_batch_id: ToolBatchId,
        call_id: String,
        status: ToolCallStatus,
        receipt_session_epoch: u64,
        receipt_step_epoch: u64,
    },
    ToolBatchSettled {
        tool_batch_id: ToolBatchId,
        results_ref: Option<String>,
    },
    LeaseIssued {
        lease: RunLease,
    },
    LeaseExpiryCheck {
        observed_time_ns: u64,
    },
    WorkspaceSyncRequested {
        workspace_binding: WorkspaceBinding,
        prompt_pack: Option<String>,
        tool_catalog: Option<String>,
        known_version: Option<u64>,
    },
    WorkspaceSyncUnchanged {
        workspace: String,
        version: Option<u64>,
    },
    WorkspaceSnapshotReady {
        snapshot: WorkspaceSnapshot,
        #[serde(
            default,
            with = "serde_bytes_opt",
            skip_serializing_if = "Option::is_none"
        )]
        prompt_pack_bytes: Option<Vec<u8>>,
        #[serde(
            default,
            with = "serde_bytes_opt",
            skip_serializing_if = "Option::is_none"
        )]
        tool_catalog_bytes: Option<Vec<u8>>,
    },
    WorkspaceSyncFailed {
        workspace: String,
        stage: String,
        detail: String,
    },
    WorkspaceApplyRequested {
        mode: WorkspaceApplyMode,
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
    Noop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionEvent {
    pub session_id: SessionId,
    pub run_id: Option<RunId>,
    pub turn_id: Option<TurnId>,
    pub step_id: Option<StepId>,
    pub session_epoch: u64,
    pub step_epoch: u64,
    pub event: SessionEventKind,
}
