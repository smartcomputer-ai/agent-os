use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::HashRef;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostMount {
    pub host_path: String,
    pub guest_path: String,
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostLocalTarget {
    #[serde(default)]
    pub mounts: Option<Vec<HostMount>>,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
    pub network_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSandboxTarget {
    pub image: String,
    #[serde(default)]
    pub runtime_class: Option<String>,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub network_mode: Option<String>,
    #[serde(default)]
    pub mounts: Option<Vec<HostMount>>,
    #[serde(default)]
    pub cpu_limit_millis: Option<u64>,
    #[serde(default)]
    pub memory_limit_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value", rename_all = "snake_case")]
pub enum HostTarget {
    Local(HostLocalTarget),
    Sandbox(HostSandboxTarget),
}

impl HostTarget {
    pub fn local(local: HostLocalTarget) -> Self {
        Self::Local(local)
    }

    pub fn sandbox(sandbox: HostSandboxTarget) -> Self {
        Self::Sandbox(sandbox)
    }

    pub fn as_local(&self) -> Option<&HostLocalTarget> {
        match self {
            Self::Local(local) => Some(local),
            Self::Sandbox(_) => None,
        }
    }

    pub fn as_sandbox(&self) -> Option<&HostSandboxTarget> {
        match self {
            Self::Local(_) => None,
            Self::Sandbox(sandbox) => Some(sandbox),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSessionOpenParams {
    pub target: HostTarget,
    #[serde(default)]
    pub session_ttl_ns: Option<u64>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSessionOpenReceipt {
    pub session_id: String,
    pub status: String,
    pub started_at_ns: u64,
    #[serde(default)]
    pub expires_at_ns: Option<u64>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostInlineText {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostInlineBytes {
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostBlobOutput {
    pub blob_ref: HashRef,
    pub size_bytes: u64,
    #[serde(default, with = "crate::serde_helpers::bytes_opt")]
    pub preview_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum HostOutput {
    InlineText { inline_text: HostInlineText },
    InlineBytes { inline_bytes: HostInlineBytes },
    Blob { blob: HostBlobOutput },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostExecParams {
    pub session_id: String,
    pub argv: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub timeout_ns: Option<u64>,
    #[serde(default)]
    pub env_patch: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub stdin_ref: Option<HashRef>,
    #[serde(default)]
    pub output_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostExecReceipt {
    pub exit_code: i32,
    pub status: String,
    #[serde(default)]
    pub stdout: Option<HostOutput>,
    #[serde(default)]
    pub stderr: Option<HostOutput>,
    pub started_at_ns: u64,
    pub ended_at_ns: u64,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostExecProgressFrame {
    #[serde(default)]
    pub exec_id: Option<String>,
    pub elapsed_ns: u64,
    #[serde(with = "serde_bytes")]
    pub stdout_delta: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub stderr_delta: Vec<u8>,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSessionSignalParams {
    pub session_id: String,
    pub signal: String,
    #[serde(default)]
    pub grace_timeout_ns: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSessionSignalReceipt {
    pub status: String,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub ended_at_ns: Option<u64>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostBlobRefInput {
    pub blob_ref: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum HostFileContentInput {
    InlineText { inline_text: HostInlineText },
    InlineBytes { inline_bytes: HostInlineBytes },
    BlobRef { blob_ref: HostBlobRefInput },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsReadFileParams {
    pub session_id: String,
    pub path: String,
    #[serde(default)]
    pub offset_bytes: Option<u64>,
    #[serde(default)]
    pub max_bytes: Option<u64>,
    #[serde(default)]
    pub encoding: Option<String>,
    #[serde(default)]
    pub output_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsReadFileReceipt {
    pub status: String,
    #[serde(default)]
    pub content: Option<HostOutput>,
    #[serde(default)]
    pub truncated: Option<bool>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub mtime_ns: Option<u64>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsWriteFileParams {
    pub session_id: String,
    pub path: String,
    pub content: HostFileContentInput,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub create_parents: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsWriteFileReceipt {
    pub status: String,
    #[serde(default)]
    pub written_bytes: Option<u64>,
    #[serde(default)]
    pub created: Option<bool>,
    #[serde(default)]
    pub new_mtime_ns: Option<u64>,
    #[serde(default)]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsEditFileParams {
    pub session_id: String,
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsEditFileReceipt {
    pub status: String,
    #[serde(default)]
    pub replacements: Option<u64>,
    #[serde(default)]
    pub applied: Option<bool>,
    #[serde(default)]
    pub summary_text: Option<String>,
    #[serde(default)]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum HostPatchInput {
    InlineText { inline_text: HostInlineText },
    BlobRef { blob_ref: HostBlobRefInput },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostPatchOpsSummary {
    pub add: u64,
    pub update: u64,
    pub delete: u64,
    #[serde(rename = "move")]
    pub r#move: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsApplyPatchParams {
    pub session_id: String,
    pub patch: HostPatchInput,
    #[serde(default)]
    pub patch_format: Option<String>,
    #[serde(default)]
    pub dry_run: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_target_roundtrips_local_and_sandbox() {
        let local = HostTarget::local(HostLocalTarget {
            mounts: None,
            workdir: None,
            env: None,
            network_mode: "none".to_string(),
        });
        let local_bytes = serde_cbor::to_vec(&local).unwrap();
        assert_eq!(
            serde_cbor::from_slice::<HostTarget>(&local_bytes).unwrap(),
            local
        );

        let sandbox = HostTarget::sandbox(HostSandboxTarget {
            image: "docker.io/library/alpine:latest".to_string(),
            runtime_class: Some("smolvm".to_string()),
            workdir: Some("/workspace".to_string()),
            env: Some(BTreeMap::from([("A".to_string(), "B".to_string())])),
            network_mode: Some("egress".to_string()),
            mounts: None,
            cpu_limit_millis: Some(1_000),
            memory_limit_bytes: Some(268_435_456),
        });
        let sandbox_bytes = serde_cbor::to_vec(&sandbox).unwrap();
        assert_eq!(
            serde_cbor::from_slice::<HostTarget>(&sandbox_bytes).unwrap(),
            sandbox
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsApplyPatchReceipt {
    pub status: String,
    #[serde(default)]
    pub files_changed: Option<u64>,
    #[serde(default)]
    pub changed_paths: Option<Vec<String>>,
    #[serde(default)]
    pub ops: Option<HostPatchOpsSummary>,
    #[serde(default)]
    pub summary_text: Option<String>,
    #[serde(default)]
    pub errors: Option<Vec<String>>,
    #[serde(default)]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum HostTextOutput {
    InlineText { inline_text: HostInlineText },
    Blob { blob: HostBlobOutput },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsGrepParams {
    pub session_id: String,
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub glob_filter: Option<String>,
    #[serde(default)]
    pub max_results: Option<u64>,
    #[serde(default)]
    pub case_insensitive: Option<bool>,
    #[serde(default)]
    pub output_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsGrepReceipt {
    pub status: String,
    #[serde(default)]
    pub matches: Option<HostTextOutput>,
    #[serde(default)]
    pub match_count: Option<u64>,
    #[serde(default)]
    pub truncated: Option<bool>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub summary_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsGlobParams {
    pub session_id: String,
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub max_results: Option<u64>,
    #[serde(default)]
    pub output_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsGlobReceipt {
    pub status: String,
    #[serde(default)]
    pub paths: Option<HostTextOutput>,
    #[serde(default)]
    pub count: Option<u64>,
    #[serde(default)]
    pub truncated: Option<bool>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub summary_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsStatParams {
    pub session_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsStatReceipt {
    pub status: String,
    #[serde(default)]
    pub exists: Option<bool>,
    #[serde(default)]
    pub is_dir: Option<bool>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub mtime_ns: Option<u64>,
    #[serde(default)]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsExistsParams {
    pub session_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsExistsReceipt {
    pub status: String,
    #[serde(default)]
    pub exists: Option<bool>,
    #[serde(default)]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsListDirParams {
    pub session_id: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub max_results: Option<u64>,
    #[serde(default)]
    pub output_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsListDirReceipt {
    pub status: String,
    #[serde(default)]
    pub entries: Option<HostTextOutput>,
    #[serde(default)]
    pub count: Option<u64>,
    #[serde(default)]
    pub truncated: Option<bool>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub summary_text: Option<String>,
}
