use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::{
    fs::WorkspaceFs,
    runtime::{ExecEventStream, FabricHostError, FabricRuntime, RuntimeInventoryEntry},
    state::{DEFAULT_WORKDIR, FabricSessionMarker, HostPaths},
};
use fabric_protocol::{
    ExecRequest, FsApplyPatchRequest, FsApplyPatchResponse, FsEditFileRequest, FsEditFileResponse,
    FsExistsResponse, FsFileReadResponse, FsFileWriteRequest, FsGlobRequest, FsGlobResponse,
    FsGrepRequest, FsGrepResponse, FsListDirResponse, FsMkdirRequest, FsPathQuery, FsRemoveRequest,
    FsRemoveResponse, FsStatResponse, FsWriteResponse, HostId, HostInfoResponse,
    HostInventoryResponse, HostInventorySession, NetworkMode, ResourceLimits, SessionId,
    SessionOpenRequest, SessionOpenResponse, SessionStatus, SessionStatusResponse,
    SignalSessionRequest,
};

use super::config::FabricHostConfig;

#[derive(Clone)]
pub struct FabricHostService {
    config: FabricHostConfig,
    runtime: Arc<dyn FabricRuntime>,
    fs: WorkspaceFs,
    paths: HostPaths,
}

impl FabricHostService {
    pub fn new(config: FabricHostConfig, runtime: Arc<dyn FabricRuntime>) -> Self {
        let paths = HostPaths::new(config.state_root.clone());
        let fs = WorkspaceFs::new(paths.clone());
        Self {
            config,
            runtime,
            fs,
            paths,
        }
    }

    pub fn config(&self) -> &FabricHostConfig {
        &self.config
    }

    pub fn host_info(&self) -> HostInfoResponse {
        HostInfoResponse {
            host_id: HostId(self.config.host_id.clone()),
            daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
            runtime_kind: "smolvm".to_owned(),
            runtime_version: None,
            state_root: self.config.state_root.to_string_lossy().into_owned(),
            bind_addr: self.config.bind_addr.to_string(),
            default_network_mode: NetworkMode::Disabled,
            allowed_network_modes: vec![NetworkMode::Disabled, NetworkMode::Egress],
            resource_defaults: ResourceLimits::default(),
            resource_max: ResourceLimits::default(),
            allowed_images: vec!["*".to_owned()],
        }
    }

    pub async fn inventory(&self) -> Result<HostInventoryResponse, FabricHostError> {
        let runtime_entries = self.runtime.inventory().await?;
        let markers = self.paths.read_all_markers()?;
        let workspace_session_ids = self.paths.workspace_session_ids()?;
        let sessions =
            reconcile_inventory(&self.paths, runtime_entries, markers, workspace_session_ids);

        Ok(HostInventoryResponse {
            host_id: HostId(self.config.host_id.clone()),
            sessions,
        })
    }

    pub async fn open_session(
        &self,
        request: SessionOpenRequest,
    ) -> Result<SessionOpenResponse, FabricHostError> {
        if request.image.trim().is_empty() {
            return Err(FabricHostError::BadRequest(
                "session image must not be empty".to_owned(),
            ));
        }

        self.runtime.open_session(request).await
    }

    pub async fn session_status(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionStatusResponse, FabricHostError> {
        self.runtime.session_status(session_id).await
    }

    pub async fn exec_stream(
        &self,
        request: ExecRequest,
    ) -> Result<ExecEventStream, FabricHostError> {
        if request.argv.is_empty() {
            return Err(FabricHostError::BadRequest(
                "exec argv must not be empty".to_owned(),
            ));
        }

        self.runtime.exec_stream(request).await
    }

    pub async fn signal_session(
        &self,
        session_id: &SessionId,
        request: SignalSessionRequest,
    ) -> Result<SessionStatusResponse, FabricHostError> {
        self.runtime.signal_session(session_id, request).await
    }

    pub fn read_file(
        &self,
        session_id: &SessionId,
        query: FsPathQuery,
    ) -> Result<FsFileReadResponse, FabricHostError> {
        self.fs.read_file(session_id, query)
    }

    pub fn write_file(
        &self,
        session_id: &SessionId,
        request: FsFileWriteRequest,
    ) -> Result<FsWriteResponse, FabricHostError> {
        self.fs.write_file(session_id, request)
    }

    pub fn edit_file(
        &self,
        session_id: &SessionId,
        request: FsEditFileRequest,
    ) -> Result<FsEditFileResponse, FabricHostError> {
        self.fs.edit_file(session_id, request)
    }

    pub fn apply_patch(
        &self,
        session_id: &SessionId,
        request: FsApplyPatchRequest,
    ) -> Result<FsApplyPatchResponse, FabricHostError> {
        self.fs.apply_patch(session_id, request)
    }

    pub fn mkdir(
        &self,
        session_id: &SessionId,
        request: FsMkdirRequest,
    ) -> Result<FsStatResponse, FabricHostError> {
        self.fs.mkdir(session_id, request)
    }

    pub fn remove(
        &self,
        session_id: &SessionId,
        request: FsRemoveRequest,
    ) -> Result<FsRemoveResponse, FabricHostError> {
        self.fs.remove(session_id, request)
    }

    pub fn exists(
        &self,
        session_id: &SessionId,
        query: FsPathQuery,
    ) -> Result<FsExistsResponse, FabricHostError> {
        self.fs.exists(session_id, query)
    }

    pub fn stat(
        &self,
        session_id: &SessionId,
        query: FsPathQuery,
    ) -> Result<FsStatResponse, FabricHostError> {
        self.fs.stat(session_id, query)
    }

    pub fn list_dir(
        &self,
        session_id: &SessionId,
        query: FsPathQuery,
    ) -> Result<FsListDirResponse, FabricHostError> {
        self.fs.list_dir(session_id, query)
    }

    pub fn grep(
        &self,
        session_id: &SessionId,
        request: FsGrepRequest,
    ) -> Result<FsGrepResponse, FabricHostError> {
        self.fs.grep(session_id, request)
    }

    pub fn glob(
        &self,
        session_id: &SessionId,
        request: FsGlobRequest,
    ) -> Result<FsGlobResponse, FabricHostError> {
        self.fs.glob(session_id, request)
    }
}

fn reconcile_inventory(
    paths: &HostPaths,
    runtime_entries: Vec<RuntimeInventoryEntry>,
    mut markers: BTreeMap<SessionId, FabricSessionMarker>,
    workspace_session_ids: Vec<SessionId>,
) -> Vec<HostInventorySession> {
    let workspace_ids: BTreeSet<SessionId> = workspace_session_ids.into_iter().collect();
    let mut sessions = Vec::new();
    let mut seen = BTreeSet::new();

    for runtime in runtime_entries {
        let marker = markers.remove(&runtime.session_id);
        let workspace_present = workspace_ids.contains(&runtime.session_id);
        seen.insert(runtime.session_id.clone());
        sessions.push(inventory_session_from_parts(
            paths,
            runtime.session_id.clone(),
            Some(runtime),
            marker,
            workspace_present,
        ));
    }

    for (session_id, marker) in markers {
        let workspace_present = workspace_ids.contains(&session_id);
        if !workspace_present {
            continue;
        }
        seen.insert(session_id.clone());
        sessions.push(inventory_session_from_parts(
            paths,
            session_id,
            None,
            Some(marker),
            workspace_present,
        ));
    }

    for session_id in workspace_ids {
        if seen.contains(&session_id) {
            continue;
        }
        sessions.push(inventory_session_from_parts(
            paths, session_id, None, None, true,
        ));
    }

    sessions.sort_by(|left, right| left.session_id.0.cmp(&right.session_id.0));
    sessions
}

fn inventory_session_from_parts(
    paths: &HostPaths,
    session_id: SessionId,
    runtime: Option<RuntimeInventoryEntry>,
    marker: Option<FabricSessionMarker>,
    workspace_present: bool,
) -> HostInventorySession {
    let status = runtime
        .as_ref()
        .map(|entry| entry.status)
        .unwrap_or(SessionStatus::OrphanedWorkspace);
    let marker_present = marker.is_some();
    let machine_name = runtime
        .as_ref()
        .map(|entry| entry.machine_name.clone())
        .or_else(|| marker.as_ref().map(|marker| marker.machine_name.clone()));
    let image = runtime
        .as_ref()
        .and_then(|entry| entry.image.clone())
        .or_else(|| marker.as_ref().map(|marker| marker.image.clone()));
    let workdir = runtime
        .as_ref()
        .and_then(|entry| entry.workdir.clone())
        .or_else(|| marker.as_ref().map(|marker| marker.workdir.clone()))
        .or_else(|| Some(DEFAULT_WORKDIR.to_owned()));
    let network_mode = runtime
        .as_ref()
        .and_then(|entry| entry.network_mode)
        .or_else(|| marker.as_ref().map(|marker| marker.network_mode));
    let workspace_path = marker
        .as_ref()
        .map(|marker| marker.workspace_path.to_string_lossy().into_owned())
        .or_else(|| {
            workspace_present.then(|| paths.workspace(&session_id).to_string_lossy().into_owned())
        });

    HostInventorySession {
        session_id,
        status,
        machine_name,
        image,
        workspace_path,
        workdir,
        network_mode,
        runtime_present: runtime.is_some(),
        workspace_present,
        marker_present,
        created_at_ns: marker.as_ref().map(|marker| marker.created_at_ns),
        expires_at_ns: marker.as_ref().and_then(|marker| marker.expires_at_ns),
        labels: marker
            .as_ref()
            .map(|marker| marker.labels.clone())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::derive_machine_name;

    fn marker(paths: &HostPaths, session_id: &SessionId) -> FabricSessionMarker {
        FabricSessionMarker {
            host_id: "host-a".to_owned(),
            session_id: session_id.clone(),
            machine_name: derive_machine_name("host-a", session_id),
            image: "alpine:latest".to_owned(),
            workspace_path: paths.workspace(session_id),
            workdir: "/workspace".to_owned(),
            network_mode: NetworkMode::Disabled,
            status: None,
            created_at_ns: 42,
            expires_at_ns: None,
            labels: BTreeMap::new(),
        }
    }

    #[test]
    fn reconcile_inventory_merges_runtime_marker_and_workspace() {
        let paths = HostPaths::new("var/test");
        let session_id = SessionId("sess-one".to_owned());
        let runtime = RuntimeInventoryEntry {
            session_id: session_id.clone(),
            machine_name: derive_machine_name("host-a", &session_id),
            status: SessionStatus::Ready,
            image: Some("alpine:latest".to_owned()),
            workdir: Some("/workspace".to_owned()),
            network_mode: Some(NetworkMode::Egress),
        };

        let sessions = reconcile_inventory(
            &paths,
            vec![runtime],
            BTreeMap::from([(session_id.clone(), marker(&paths, &session_id))]),
            vec![session_id.clone()],
        );

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, session_id);
        assert_eq!(sessions[0].status, SessionStatus::Ready);
        assert!(sessions[0].runtime_present);
        assert!(sessions[0].marker_present);
        assert!(sessions[0].workspace_present);
    }

    #[test]
    fn reconcile_inventory_reports_marker_workspace_without_runtime_as_orphan() {
        let paths = HostPaths::new("var/test");
        let session_id = SessionId("sess-orphan".to_owned());
        let sessions = reconcile_inventory(
            &paths,
            Vec::new(),
            BTreeMap::from([(session_id.clone(), marker(&paths, &session_id))]),
            vec![session_id.clone()],
        );

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, session_id);
        assert_eq!(sessions[0].status, SessionStatus::OrphanedWorkspace);
        assert!(!sessions[0].runtime_present);
        assert!(sessions[0].marker_present);
        assert!(sessions[0].workspace_present);
    }

    #[test]
    fn reconcile_inventory_reports_workspace_without_marker_as_orphan() {
        let paths = HostPaths::new("var/test");
        let session_id = SessionId("sess-workspace-only".to_owned());
        let sessions = reconcile_inventory(
            &paths,
            Vec::new(),
            BTreeMap::new(),
            vec![session_id.clone()],
        );

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, session_id);
        assert_eq!(sessions[0].status, SessionStatus::OrphanedWorkspace);
        assert!(!sessions[0].runtime_present);
        assert!(!sessions[0].marker_present);
        assert!(sessions[0].workspace_present);
    }

    #[test]
    fn reconcile_inventory_skips_marker_without_workspace_or_runtime() {
        let paths = HostPaths::new("var/test");
        let session_id = SessionId("sess-marker-only".to_owned());
        let sessions = reconcile_inventory(
            &paths,
            Vec::new(),
            BTreeMap::from([(session_id.clone(), marker(&paths, &session_id))]),
            Vec::new(),
        );

        assert!(sessions.is_empty());
    }
}
