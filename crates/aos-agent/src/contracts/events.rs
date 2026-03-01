use super::{
    HostCommand, HostSessionStatus, SessionConfig, SessionId, ToolOverrideScope, ToolSpec,
    WorkspaceApplyMode, WorkspaceBinding, WorkspaceSnapshot,
};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use aos_wasm_sdk::EffectReceiptEnvelope;
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

mod serde_bytes_vec {
    use super::*;
    use serde_bytes::ByteBuf;

    pub fn serialize<S>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(value)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        ByteBuf::deserialize(deserializer).map(ByteBuf::into_vec)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WorkspaceSnapshotReady {
    pub snapshot: WorkspaceSnapshot,
    #[serde(
        default,
        with = "serde_bytes_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub prompt_pack_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EffectReceiptRejected {
    pub origin_module_id: String,
    #[serde(
        default,
        with = "serde_bytes_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub origin_instance_key: Option<Vec<u8>>,
    pub intent_id: String,
    pub effect_kind: String,
    pub params_hash: Option<String>,
    pub adapter_id: String,
    pub status: String,
    pub error_code: String,
    pub error_message: String,
    pub payload_hash: String,
    pub payload_size: u64,
    pub emitted_at_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EffectStreamFrameEnvelope {
    pub origin_module_id: String,
    #[serde(
        default,
        with = "serde_bytes_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub origin_instance_key: Option<Vec<u8>>,
    pub intent_id: String,
    pub effect_kind: String,
    pub params_hash: Option<String>,
    pub emitted_at_seq: u64,
    pub seq: u64,
    pub kind: String,
    #[serde(with = "serde_bytes_vec")]
    pub payload: Vec<u8>,
    pub payload_ref: Option<String>,
    pub adapter_id: String,
    #[serde(with = "serde_bytes_vec")]
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "$tag", content = "$value")]
pub enum SessionIngressKind {
    RunRequested {
        input_ref: String,
        run_overrides: Option<SessionConfig>,
    },
    HostCommandReceived(HostCommand),
    WorkspaceSyncRequested {
        workspace_binding: WorkspaceBinding,
        prompt_pack: Option<String>,
    },
    WorkspaceSyncUnchanged {
        workspace: String,
        version: Option<u64>,
    },
    WorkspaceSnapshotReady(WorkspaceSnapshotReady),
    WorkspaceSyncFailed {
        workspace: String,
        stage: String,
        detail: String,
    },
    WorkspaceApplyRequested {
        mode: WorkspaceApplyMode,
    },
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
    Noop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionIngress {
    pub session_id: SessionId,
    pub observed_at_ns: u64,
    pub ingress: SessionIngressKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "$tag", content = "$value")]
pub enum SessionWorkflowEvent {
    Ingress(SessionIngress),
    Receipt(EffectReceiptEnvelope),
    ReceiptRejected(EffectReceiptRejected),
    StreamFrame(EffectStreamFrameEnvelope),
    #[default]
    Noop,
}
