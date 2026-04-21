use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::runtime::FabricHostError;
use fabric_protocol::{NetworkMode, SessionId, SessionStatus};

pub const DEFAULT_WORKDIR: &str = "/workspace";

#[derive(Debug, Clone)]
pub struct HostPaths {
    state_root: PathBuf,
}

impl HostPaths {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        Self {
            state_root: state_root.into(),
        }
    }

    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    pub fn sessions_root(&self) -> PathBuf {
        self.state_root.join("sessions")
    }

    pub fn session_root(&self, session_id: &SessionId) -> PathBuf {
        self.sessions_root().join(&session_id.0)
    }

    pub fn session_tmp(&self, session_id: &SessionId) -> PathBuf {
        self.session_root(session_id).join("tmp")
    }

    pub fn session_logs(&self, session_id: &SessionId) -> PathBuf {
        self.session_root(session_id).join("logs")
    }

    pub fn workspace(&self, session_id: &SessionId) -> PathBuf {
        self.session_root(session_id).join("workspace")
    }

    pub fn marker_path(&self, session_id: &SessionId) -> PathBuf {
        self.session_root(session_id).join("fabric-session.json")
    }

    pub fn smolvm_root(&self) -> PathBuf {
        self.state_root.join("smolvm")
    }

    pub fn smolvm_db_path(&self) -> PathBuf {
        self.smolvm_root().join("smolvm.redb")
    }

    pub fn ensure_session_dirs(&self, session_id: &SessionId) -> Result<(), FabricHostError> {
        for path in [
            self.workspace(session_id),
            self.session_tmp(session_id),
            self.session_logs(session_id),
        ] {
            std::fs::create_dir_all(&path).map_err(|error| {
                FabricHostError::Runtime(format!(
                    "create session directory '{}': {error}",
                    path.display()
                ))
            })?;
        }
        Ok(())
    }

    pub fn write_marker(&self, marker: &FabricSessionMarker) -> Result<(), FabricHostError> {
        let path = self.marker_path(&marker.session_id);
        let tmp_path = path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(marker).map_err(|error| {
            FabricHostError::Runtime(format!(
                "serialize session marker '{}': {error}",
                marker.session_id.0
            ))
        })?;
        std::fs::write(&tmp_path, data).map_err(|error| {
            FabricHostError::Runtime(format!(
                "write session marker temp '{}': {error}",
                tmp_path.display()
            ))
        })?;
        std::fs::rename(&tmp_path, &path).map_err(|error| {
            FabricHostError::Runtime(format!(
                "commit session marker '{}': {error}",
                path.display()
            ))
        })?;
        Ok(())
    }

    pub fn read_marker(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<FabricSessionMarker>, FabricHostError> {
        let path = self.marker_path(session_id);
        let data = match std::fs::read(&path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(FabricHostError::Runtime(format!(
                    "read session marker '{}': {error}",
                    path.display()
                )));
            }
        };
        serde_json::from_slice(&data).map(Some).map_err(|error| {
            FabricHostError::Runtime(format!(
                "parse session marker '{}': {error}",
                path.display()
            ))
        })
    }

    pub fn read_all_markers(
        &self,
    ) -> Result<BTreeMap<SessionId, FabricSessionMarker>, FabricHostError> {
        let mut markers = BTreeMap::new();
        let sessions_root = self.sessions_root();
        let entries = match std::fs::read_dir(&sessions_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(markers),
            Err(error) => {
                return Err(FabricHostError::Runtime(format!(
                    "read sessions root '{}': {error}",
                    sessions_root.display()
                )));
            }
        };

        for entry in entries {
            let entry = entry.map_err(|error| {
                FabricHostError::Runtime(format!(
                    "read sessions root entry '{}': {error}",
                    sessions_root.display()
                ))
            })?;
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let session_id = SessionId(entry.file_name().to_string_lossy().into_owned());
            if let Some(marker) = self.read_marker(&session_id)? {
                markers.insert(marker.session_id.clone(), marker);
            }
        }

        Ok(markers)
    }

    pub fn workspace_session_ids(&self) -> Result<Vec<SessionId>, FabricHostError> {
        let mut session_ids = Vec::new();
        let sessions_root = self.sessions_root();
        let entries = match std::fs::read_dir(&sessions_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(session_ids),
            Err(error) => {
                return Err(FabricHostError::Runtime(format!(
                    "read sessions root '{}': {error}",
                    sessions_root.display()
                )));
            }
        };

        for entry in entries {
            let entry = entry.map_err(|error| {
                FabricHostError::Runtime(format!(
                    "read sessions root entry '{}': {error}",
                    sessions_root.display()
                ))
            })?;
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let session_id = SessionId(entry.file_name().to_string_lossy().into_owned());
            if self.workspace(&session_id).is_dir() {
                session_ids.push(session_id);
            }
        }
        session_ids.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(session_ids)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FabricSessionMarker {
    pub host_id: String,
    pub session_id: SessionId,
    pub machine_name: String,
    pub image: String,
    pub workspace_path: PathBuf,
    pub workdir: String,
    pub network_mode: NetworkMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SessionStatus>,
    pub created_at_ns: u128,
    pub expires_at_ns: Option<u128>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

pub fn generate_session_id() -> SessionId {
    SessionId(format!("sess-{}", uuid::Uuid::new_v4()))
}

pub fn derive_machine_name(host_id: &str, session_id: &SessionId) -> String {
    format!(
        "{}{}",
        derive_machine_prefix(host_id),
        sanitize_machine_component(&session_id.0)
    )
}

pub fn derive_machine_prefix(host_id: &str) -> String {
    format!("fabric-{}-", short_host_hash(host_id))
}

pub fn now_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn short_host_hash(host_id: &str) -> String {
    let digest = Sha256::digest(host_id.as_bytes());
    hex::encode(&digest[..6])
}

fn sanitize_machine_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}
