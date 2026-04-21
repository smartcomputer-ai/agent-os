#[cfg(feature = "smolvm-runtime")]
use std::path::Path;
use std::path::PathBuf;
#[cfg(feature = "smolvm-runtime")]
use std::time::Duration;

use async_trait::async_trait;
#[cfg(feature = "smolvm-runtime")]
use futures_util::stream;
#[cfg(feature = "smolvm-runtime")]
use smolvm_protocol::{AgentRequest, AgentResponse};
#[cfg(feature = "smolvm-runtime")]
use tokio::sync::mpsc;

use crate::{
    runtime::{ExecEventStream, FabricHostError, FabricRuntime, RuntimeInventoryEntry},
    state::HostPaths,
};
use fabric_protocol::{
    ExecRequest, SessionId, SessionOpenRequest, SessionOpenResponse, SessionStatusResponse,
    SignalSessionRequest,
};

#[cfg(feature = "smolvm-runtime")]
use crate::state::{
    DEFAULT_WORKDIR, FabricSessionMarker, derive_machine_name, derive_machine_prefix,
    generate_session_id, now_ns,
};
#[cfg(feature = "smolvm-runtime")]
use fabric_protocol::SessionSignal;
#[cfg(feature = "smolvm-runtime")]
use fabric_protocol::{ExecEvent, ExecEventKind, ExecId, FabricBytes};
#[cfg(feature = "smolvm-runtime")]
use fabric_protocol::{HostId, NetworkMode, SessionStatus};

#[derive(Debug, Clone)]
pub struct SmolvmRuntimeConfig {
    pub state_root: PathBuf,
    pub host_id: String,
}

#[derive(Debug)]
pub struct SmolvmRuntime {
    config: SmolvmRuntimeConfig,
    paths: HostPaths,
    #[cfg(feature = "smolvm-runtime")]
    db: smolvm::SmolvmDb,
}

impl SmolvmRuntime {
    pub fn open(config: SmolvmRuntimeConfig) -> Result<Self, FabricHostError> {
        let paths = HostPaths::new(config.state_root.clone());

        #[cfg(feature = "smolvm-runtime")]
        {
            std::fs::create_dir_all(paths.smolvm_root()).map_err(|error| {
                FabricHostError::Runtime(format!(
                    "create smolvm state root '{}': {error}",
                    paths.smolvm_root().display()
                ))
            })?;

            let db_path = paths.smolvm_db_path();
            let db = smolvm::SmolvmDb::open_at(&db_path).map_err(|error| {
                FabricHostError::Runtime(format!(
                    "open smolvm database '{}': {error}",
                    db_path.display()
                ))
            })?;
            db.init_tables().map_err(|error| {
                FabricHostError::Runtime(format!(
                    "initialize smolvm database '{}': {error}",
                    db_path.display()
                ))
            })?;

            return Ok(Self { config, paths, db });
        }

        #[cfg(not(feature = "smolvm-runtime"))]
        {
            Ok(Self { config, paths })
        }
    }

    pub fn config(&self) -> &SmolvmRuntimeConfig {
        &self.config
    }

    #[cfg(feature = "smolvm-runtime")]
    pub fn db(&self) -> &smolvm::SmolvmDb {
        &self.db
    }

    pub fn paths(&self) -> &HostPaths {
        &self.paths
    }
}

#[cfg(feature = "smolvm-runtime")]
pub fn boot_vm_from_config_path(config_path: impl AsRef<Path>) -> Result<(), FabricHostError> {
    use smolvm::agent::{LaunchConfig, VmDisks, boot_config::BootConfig, launch_agent_vm};

    unsafe {
        libc::setsid();
    }

    let config_path = config_path.as_ref();
    let config_data = std::fs::read(config_path).map_err(|error| {
        FabricHostError::Runtime(format!(
            "read smolvm boot config '{}': {error}",
            config_path.display()
        ))
    })?;
    let config: BootConfig = serde_json::from_slice(&config_data).map_err(|error| {
        FabricHostError::Runtime(format!(
            "parse smolvm boot config '{}': {error}",
            config_path.display()
        ))
    })?;

    let _ = std::fs::remove_file(config_path);
    smolvm::process::detach_stdio();

    close_inherited_fds();

    let storage_disk = smolvm::storage::StorageDisk::open_or_create_at(
        &config.storage_disk_path,
        config.storage_size_gb,
    )
    .map_err(|error| {
        let _ = std::fs::write(
            &config.startup_error_log,
            format!("failed to open storage disk: {error}"),
        );
        FabricHostError::Runtime(format!("open smolvm storage disk: {error}"))
    })?;

    let overlay_disk = smolvm::storage::OverlayDisk::open_or_create_at(
        &config.overlay_disk_path,
        config.overlay_size_gb,
    )
    .map_err(|error| {
        let _ = std::fs::write(
            &config.startup_error_log,
            format!("failed to open overlay disk: {error}"),
        );
        FabricHostError::Runtime(format!("open smolvm overlay disk: {error}"))
    })?;

    let dns_filter_socket_path = if let Some(ref hosts) = config.dns_filter_hosts {
        if hosts.is_empty() {
            None
        } else {
            let socket_path = config
                .vsock_socket
                .parent()
                .unwrap_or_else(|| Path::new("/tmp"))
                .join("dns-filter.sock");
            match smolvm::dns_filter_listener::start(&socket_path, hosts.clone()) {
                Ok(()) => Some(socket_path),
                Err(error) => {
                    tracing::warn!(%error, "failed to start smolvm DNS filter listener");
                    None
                }
            }
        }
    } else {
        None
    };

    let disks = VmDisks {
        storage: &storage_disk,
        overlay: Some(&overlay_disk),
    };

    let result = launch_agent_vm(&LaunchConfig {
        rootfs_path: &config.rootfs_path,
        disks: &disks,
        vsock_socket: &config.vsock_socket,
        console_log: config.console_log.as_deref(),
        mounts: &config.mounts,
        port_mappings: &config.ports,
        resources: config.resources,
        ssh_agent_socket: config.ssh_agent_socket.as_deref(),
        dns_filter_socket: dns_filter_socket_path.as_deref(),
        packed_layers_dir: config.packed_layers_dir.as_deref(),
        extra_disks: &config.extra_disks,
    });

    if let Err(error) = &result {
        let _ = std::fs::write(&config.startup_error_log, error.to_string());
    }

    result.map_err(|error| FabricHostError::Runtime(format!("launch smolvm agent VM: {error}")))
}

#[cfg(not(feature = "smolvm-runtime"))]
pub fn boot_vm_from_config_path(
    config_path: impl AsRef<std::path::Path>,
) -> Result<(), FabricHostError> {
    let _ = config_path;
    Err(FabricHostError::NotImplemented(
        "smolvm VM boot requires the smolvm-runtime feature",
    ))
}

#[cfg(feature = "smolvm-runtime")]
fn close_inherited_fds() {
    unsafe {
        let max_fd = libc::getdtablesize();
        for fd in 3..max_fd {
            libc::close(fd);
        }
    }
}

#[async_trait]
impl FabricRuntime for SmolvmRuntime {
    async fn open_session(
        &self,
        request: SessionOpenRequest,
    ) -> Result<SessionOpenResponse, FabricHostError> {
        #[cfg(feature = "smolvm-runtime")]
        {
            return self.open_session_with_smolvm(request).await;
        }

        #[cfg(not(feature = "smolvm-runtime"))]
        {
            let _ = request;
            Err(FabricHostError::NotImplemented(
                "smolvm session creation requires the smolvm-runtime feature",
            ))
        }
    }

    async fn session_status(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionStatusResponse, FabricHostError> {
        #[cfg(feature = "smolvm-runtime")]
        {
            return self.session_status_with_smolvm(session_id).await;
        }

        #[cfg(not(feature = "smolvm-runtime"))]
        {
            let _ = session_id;
            Err(FabricHostError::NotImplemented(
                "smolvm session inspection requires the smolvm-runtime feature",
            ))
        }
    }

    async fn exec_stream(&self, request: ExecRequest) -> Result<ExecEventStream, FabricHostError> {
        #[cfg(feature = "smolvm-runtime")]
        {
            return self.exec_stream_with_smolvm(request).await;
        }

        #[cfg(not(feature = "smolvm-runtime"))]
        {
            let _ = request;
            Err(FabricHostError::NotImplemented(
                "smolvm exec requires the smolvm-runtime feature",
            ))
        }
    }

    async fn signal_session(
        &self,
        session_id: &SessionId,
        request: SignalSessionRequest,
    ) -> Result<SessionStatusResponse, FabricHostError> {
        #[cfg(feature = "smolvm-runtime")]
        {
            return self.signal_session_with_smolvm(session_id, request).await;
        }

        #[cfg(not(feature = "smolvm-runtime"))]
        {
            let _ = (session_id, request);
            Err(FabricHostError::NotImplemented(
                "smolvm session signals require the smolvm-runtime feature",
            ))
        }
    }

    async fn inventory(&self) -> Result<Vec<RuntimeInventoryEntry>, FabricHostError> {
        #[cfg(feature = "smolvm-runtime")]
        {
            return self.inventory_with_smolvm().await;
        }

        #[cfg(not(feature = "smolvm-runtime"))]
        {
            Err(FabricHostError::NotImplemented(
                "smolvm inventory requires the smolvm-runtime feature",
            ))
        }
    }
}

#[cfg(feature = "smolvm-runtime")]
impl SmolvmRuntime {
    async fn open_session_with_smolvm(
        &self,
        request: SessionOpenRequest,
    ) -> Result<SessionOpenResponse, FabricHostError> {
        validate_open_request(&request)?;

        let session_id = request
            .session_id
            .clone()
            .unwrap_or_else(generate_session_id);
        let machine_name = derive_machine_name(&self.config.host_id, &session_id);
        let workdir = request
            .workdir
            .clone()
            .unwrap_or_else(|| DEFAULT_WORKDIR.to_owned());
        let created_at_ns = now_ns();
        let expires_at_ns = request
            .ttl_secs
            .map(|ttl_secs| created_at_ns + u128::from(ttl_secs) * 1_000_000_000);

        self.paths.ensure_session_dirs(&session_id)?;

        if self
            .db
            .get_vm(&machine_name)
            .map_err(map_smolvm_error)?
            .is_some()
        {
            return Err(FabricHostError::Conflict(format!(
                "session '{}' already exists",
                session_id.0
            )));
        }

        let marker = FabricSessionMarker {
            host_id: self.config.host_id.clone(),
            session_id: session_id.clone(),
            machine_name: machine_name.clone(),
            image: request.image.clone(),
            workspace_path: self.canonical_session_workspace(&session_id)?,
            workdir: workdir.clone(),
            network_mode: request.network_mode,
            status: None,
            created_at_ns,
            expires_at_ns,
            labels: request.labels.clone(),
        };
        self.paths.write_marker(&marker)?;

        let mut record = smolvm::VmRecord::new(
            machine_name.clone(),
            cpu_limit_to_vcpus(request.resources.cpu_limit_millis),
            memory_limit_to_mib(request.resources.memory_limit_bytes),
            vec![(
                marker.workspace_path.to_string_lossy().into_owned(),
                DEFAULT_WORKDIR.to_owned(),
                false,
            )],
            Vec::new(),
            request.network_mode == NetworkMode::Egress,
        );
        record.image = Some(request.image.clone());
        record.workdir = Some(workdir.clone());
        record.env = request.env.into_iter().collect();
        record.ephemeral = false;

        let inserted = self
            .db
            .insert_vm_if_not_exists(&machine_name, &record)
            .map_err(map_smolvm_error)?;
        if !inserted {
            return Err(FabricHostError::Conflict(format!(
                "session '{}' already exists",
                session_id.0
            )));
        }

        start_machine_and_pull_image(self.db.clone(), machine_name.clone(), request.image).await?;

        Ok(SessionOpenResponse {
            session_id,
            status: SessionStatus::Ready,
            workdir,
            host_id: Some(HostId(self.config.host_id.clone())),
        })
    }

    fn canonical_session_workspace(
        &self,
        session_id: &SessionId,
    ) -> Result<PathBuf, FabricHostError> {
        let workspace = self.paths.workspace(session_id);
        workspace.canonicalize().map_err(|error| {
            FabricHostError::Runtime(format!(
                "canonicalize session workspace '{}': {error}",
                workspace.display()
            ))
        })
    }

    async fn session_status_with_smolvm(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionStatusResponse, FabricHostError> {
        let machine_name = derive_machine_name(&self.config.host_id, session_id);
        if let Some(record) = self.db.get_vm(&machine_name).map_err(map_smolvm_error)? {
            return Ok(SessionStatusResponse {
                session_id: session_id.clone(),
                status: record_state_to_session_status(record.actual_state()),
            });
        }

        if let Some(marker) = self.paths.read_marker(session_id)? {
            return Ok(SessionStatusResponse {
                session_id: session_id.clone(),
                status: marker.status.unwrap_or(SessionStatus::OrphanedWorkspace),
            });
        }

        Err(FabricHostError::NotFound(format!(
            "session '{}'",
            session_id.0
        )))
    }

    async fn exec_stream_with_smolvm(
        &self,
        request: ExecRequest,
    ) -> Result<ExecEventStream, FabricHostError> {
        let machine_name = derive_machine_name(&self.config.host_id, &request.session_id);
        let record = self
            .db
            .get_vm(&machine_name)
            .map_err(map_smolvm_error)?
            .ok_or_else(|| {
                FabricHostError::NotFound(format!("session '{}'", request.session_id.0))
            })?;
        let image = record.image.clone().ok_or_else(|| {
            FabricHostError::Runtime(format!("session '{}' has no image", request.session_id.0))
        })?;
        let default_workdir = self
            .paths
            .read_marker(&request.session_id)?
            .map(|marker| marker.workdir)
            .or(record.workdir.clone())
            .unwrap_or_else(|| DEFAULT_WORKDIR.to_owned());

        exec_machine_stream(
            machine_name,
            image,
            record.mounts.clone(),
            request.argv,
            request.cwd.or(Some(default_workdir)),
            request.env.into_iter().collect(),
            request.stdin,
            request.timeout_secs,
        )
        .await
    }

    async fn signal_session_with_smolvm(
        &self,
        session_id: &SessionId,
        request: SignalSessionRequest,
    ) -> Result<SessionStatusResponse, FabricHostError> {
        let machine_name = derive_machine_name(&self.config.host_id, session_id);
        let record = self
            .db
            .get_vm(&machine_name)
            .map_err(map_smolvm_error)?
            .ok_or_else(|| FabricHostError::NotFound(format!("session '{}'", session_id.0)))?;

        match request.action {
            SessionSignal::Resume => {
                let image = record.image.clone().ok_or_else(|| {
                    FabricHostError::Runtime(format!("session '{}' has no image", session_id.0))
                })?;
                start_machine_and_pull_image(self.db.clone(), machine_name, image).await?;
                Ok(SessionStatusResponse {
                    session_id: session_id.clone(),
                    status: SessionStatus::Ready,
                })
            }
            SessionSignal::Quiesce | SessionSignal::Terminate => {
                stop_machine(self.db.clone(), machine_name).await?;
                Ok(SessionStatusResponse {
                    session_id: session_id.clone(),
                    status: SessionStatus::Quiesced,
                })
            }
            SessionSignal::Close => {
                stop_machine(self.db.clone(), machine_name.clone()).await?;
                self.db.remove_vm(&machine_name).map_err(map_smolvm_error)?;
                if let Some(mut marker) = self.paths.read_marker(session_id)? {
                    marker.status = Some(SessionStatus::Closed);
                    self.paths.write_marker(&marker)?;
                }
                Ok(SessionStatusResponse {
                    session_id: session_id.clone(),
                    status: SessionStatus::Closed,
                })
            }
        }
    }

    async fn inventory_with_smolvm(&self) -> Result<Vec<RuntimeInventoryEntry>, FabricHostError> {
        let db = self.db.clone();
        let host_id = self.config.host_id.clone();
        tokio::task::spawn_blocking(move || {
            let prefix = derive_machine_prefix(&host_id);
            let mut entries = Vec::new();
            for (machine_name, record) in db.list_vms().map_err(map_smolvm_error)? {
                let Some(session_suffix) = machine_name.strip_prefix(&prefix) else {
                    continue;
                };
                entries.push(RuntimeInventoryEntry {
                    session_id: SessionId(session_suffix.to_owned()),
                    machine_name,
                    status: record_state_to_session_status(record.actual_state()),
                    image: record.image.clone(),
                    workdir: record.workdir.clone(),
                    network_mode: Some(if record.network {
                        NetworkMode::Egress
                    } else {
                        NetworkMode::Disabled
                    }),
                });
            }
            entries.sort_by(|left, right| left.session_id.0.cmp(&right.session_id.0));
            Ok(entries)
        })
        .await
        .map_err(|error| FabricHostError::Runtime(format!("join smolvm inventory task: {error}")))?
    }
}

#[cfg(feature = "smolvm-runtime")]
async fn exec_machine_stream(
    machine_name: String,
    image: String,
    mounts: Vec<(String, String, bool)>,
    argv: Vec<String>,
    workdir: Option<String>,
    env: Vec<(String, String)>,
    stdin: Option<FabricBytes>,
    timeout_secs: Option<u64>,
) -> Result<ExecEventStream, FabricHostError> {
    let stdin_bytes = stdin
        .map(|stdin| stdin.decode_bytes().map_err(FabricHostError::BadRequest))
        .transpose()?;
    let manager = smolvm::AgentManager::for_vm(&machine_name).map_err(map_smolvm_error)?;
    if manager.try_connect_existing().is_none() {
        return Err(FabricHostError::Conflict(format!(
            "session machine '{machine_name}' is not running"
        )));
    }
    manager.detach();

    let (sender, receiver) = mpsc::channel(64);
    std::thread::Builder::new()
        .name(format!("fabric-exec-{machine_name}"))
        .spawn(move || {
            if let Err(error) = exec_machine_stream_blocking(
                machine_name,
                image,
                mounts,
                argv,
                workdir,
                env,
                stdin_bytes,
                timeout_secs,
                sender.clone(),
            ) {
                let _ = sender.blocking_send(Err(error));
            }
        })
        .map_err(|error| FabricHostError::Runtime(format!("spawn smolvm exec worker: {error}")))?;

    Ok(Box::pin(stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    })))
}

#[cfg(feature = "smolvm-runtime")]
fn exec_machine_stream_blocking(
    machine_name: String,
    image: String,
    mounts: Vec<(String, String, bool)>,
    argv: Vec<String>,
    workdir: Option<String>,
    env: Vec<(String, String)>,
    stdin_bytes: Option<Vec<u8>>,
    timeout_secs: Option<u64>,
    sender: mpsc::Sender<Result<ExecEvent, FabricHostError>>,
) -> Result<(), FabricHostError> {
    let exec_id = ExecId(format!("exec-{}", uuid::Uuid::new_v4()));
    let mut seq = 0;
    let manager = smolvm::AgentManager::for_vm(&machine_name).map_err(map_smolvm_error)?;
    let result = (|| {
        let mut client = manager.connect().map_err(map_smolvm_error)?;
        let _read_timeout_guard = client
            .set_extended_read_timeout(exec_read_timeout(timeout_secs))
            .map_err(map_smolvm_error)?;

        client
            .send_raw(&AgentRequest::Run {
                image,
                command: argv,
                env,
                workdir,
                mounts: record_mounts_to_runconfig_bindings(&mounts),
                timeout_ms: timeout_secs.map(|seconds| seconds.saturating_mul(1_000)),
                interactive: true,
                tty: false,
                persistent_overlay_id: Some(machine_name.clone()),
            })
            .map_err(map_smolvm_error)?;

        match client.recv_raw().map_err(map_smolvm_error)? {
            AgentResponse::Started => {
                if !send_exec_event(
                    &sender,
                    &exec_id,
                    &mut seq,
                    ExecEventKind::Started,
                    None,
                    None,
                    None,
                ) {
                    return Ok(());
                }
            }
            AgentResponse::Error { message, .. } => {
                send_exec_event(
                    &sender,
                    &exec_id,
                    &mut seq,
                    ExecEventKind::Error,
                    None,
                    None,
                    Some(message),
                );
                return Ok(());
            }
            response => {
                return Err(FabricHostError::Runtime(format!(
                    "expected smolvm Started response, got {response:?}"
                )));
            }
        }

        if let Some(stdin_bytes) = stdin_bytes
            && !stdin_bytes.is_empty()
        {
            client
                .send_raw(&AgentRequest::Stdin { data: stdin_bytes })
                .map_err(map_smolvm_error)?;
        }
        client
            .send_raw(&AgentRequest::Stdin { data: Vec::new() })
            .map_err(map_smolvm_error)?;

        loop {
            match client.recv_raw().map_err(map_smolvm_error)? {
                AgentResponse::Stdout { data } => {
                    if !send_exec_event(
                        &sender,
                        &exec_id,
                        &mut seq,
                        ExecEventKind::Stdout,
                        Some(FabricBytes::from_bytes_auto(data)),
                        None,
                        None,
                    ) {
                        break;
                    }
                }
                AgentResponse::Stderr { data } => {
                    if !send_exec_event(
                        &sender,
                        &exec_id,
                        &mut seq,
                        ExecEventKind::Stderr,
                        Some(FabricBytes::from_bytes_auto(data)),
                        None,
                        None,
                    ) {
                        break;
                    }
                }
                AgentResponse::Exited { exit_code } => {
                    send_exec_event(
                        &sender,
                        &exec_id,
                        &mut seq,
                        ExecEventKind::Exit,
                        None,
                        Some(exit_code),
                        None,
                    );
                    break;
                }
                AgentResponse::Error { message, .. } => {
                    send_exec_event(
                        &sender,
                        &exec_id,
                        &mut seq,
                        ExecEventKind::Error,
                        None,
                        None,
                        Some(message),
                    );
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    })();
    manager.detach();
    result
}

#[cfg(feature = "smolvm-runtime")]
fn send_exec_event(
    sender: &mpsc::Sender<Result<ExecEvent, FabricHostError>>,
    exec_id: &ExecId,
    seq: &mut u64,
    kind: ExecEventKind,
    data: Option<FabricBytes>,
    exit_code: Option<i32>,
    message: Option<String>,
) -> bool {
    let event = ExecEvent {
        exec_id: exec_id.clone(),
        seq: *seq,
        kind,
        data,
        exit_code,
        message,
    };
    *seq += 1;
    sender.blocking_send(Ok(event)).is_ok()
}

#[cfg(feature = "smolvm-runtime")]
fn exec_read_timeout(timeout_secs: Option<u64>) -> Duration {
    match timeout_secs {
        Some(seconds) => Duration::from_secs(seconds.saturating_add(5).max(5)),
        None => Duration::from_secs(24 * 60 * 60),
    }
}

#[cfg(feature = "smolvm-runtime")]
async fn stop_machine(db: smolvm::SmolvmDb, machine_name: String) -> Result<(), FabricHostError> {
    tokio::task::spawn_blocking(move || {
        let record = db
            .get_vm(&machine_name)
            .map_err(map_smolvm_error)?
            .ok_or_else(|| FabricHostError::NotFound(format!("machine '{machine_name}'")))?;

        let manager = smolvm::AgentManager::for_vm_with_sizes(
            &machine_name,
            record.storage_gb,
            record.overlay_gb,
        )
        .map_err(map_smolvm_error)?;
        manager.stop().map_err(map_smolvm_error)?;

        db.update_vm(&machine_name, |record| {
            record.state = smolvm::RecordState::Stopped;
            record.pid = None;
            record.pid_start_time = None;
        })
        .map_err(map_smolvm_error)?;

        Ok(())
    })
    .await
    .map_err(|error| FabricHostError::Runtime(format!("join smolvm stop task: {error}")))?
}

#[cfg(feature = "smolvm-runtime")]
fn record_mounts_to_runconfig_bindings(
    mounts: &[(String, String, bool)],
) -> Vec<(String, String, bool)> {
    mounts
        .iter()
        .enumerate()
        .map(|(index, (_host, target, read_only))| {
            (
                smolvm::HostMount::mount_tag(index),
                target.clone(),
                *read_only,
            )
        })
        .collect()
}

#[cfg(feature = "smolvm-runtime")]
async fn start_machine_and_pull_image(
    db: smolvm::SmolvmDb,
    machine_name: String,
    image: String,
) -> Result<(), FabricHostError> {
    tokio::task::spawn_blocking(move || {
        let record = db
            .get_vm(&machine_name)
            .map_err(map_smolvm_error)?
            .ok_or_else(|| FabricHostError::NotFound(format!("machine '{machine_name}'")))?;

        let manager = smolvm::AgentManager::for_vm_with_sizes(
            &machine_name,
            record.storage_gb,
            record.overlay_gb,
        )
        .map_err(map_smolvm_error)?;

        manager
            .ensure_running_via_subprocess(
                record.host_mounts(),
                record.port_mappings(),
                record.vm_resources(),
                Default::default(),
            )
            .map_err(map_smolvm_error)?;

        let pid = manager.child_pid();
        db.update_vm(&machine_name, |record| {
            record.state = smolvm::RecordState::Running;
            record.pid = pid;
            record.pid_start_time = pid.and_then(smolvm::process::process_start_time);
        })
        .map_err(map_smolvm_error)?;

        let mut client = manager.connect().map_err(map_smolvm_error)?;
        client
            .pull_with_registry_config(&image)
            .map_err(map_smolvm_error)?;
        remove_storage_workspace_shadow(&mut client)?;

        manager.detach();

        Ok(())
    })
    .await
    .map_err(|error| FabricHostError::Runtime(format!("join smolvm startup task: {error}")))?
}

#[cfg(feature = "smolvm-runtime")]
fn remove_storage_workspace_shadow(
    client: &mut smolvm::agent::AgentClient,
) -> Result<(), FabricHostError> {
    // The current smolvm agent bind-mounts /storage/workspace over /workspace
    // for image exec. Fabric owns /workspace as a host-backed virtiofs mount,
    // so remove the default storage directory after each VM start.
    let (exit_code, _stdout, stderr) = client
        .vm_exec(
            vec![
                "/bin/sh".to_owned(),
                "-lc".to_owned(),
                "rm -rf /storage/workspace".to_owned(),
            ],
            Vec::new(),
            Some("/".to_owned()),
            Some(std::time::Duration::from_secs(10)),
        )
        .map_err(map_smolvm_error)?;

    if exit_code != 0 {
        return Err(FabricHostError::Runtime(format!(
            "remove smolvm default workspace shadow failed with exit code {exit_code}: {}",
            String::from_utf8_lossy(&stderr)
        )));
    }

    Ok(())
}

#[cfg(feature = "smolvm-runtime")]
fn validate_open_request(request: &SessionOpenRequest) -> Result<(), FabricHostError> {
    if request.image.trim().is_empty() {
        return Err(FabricHostError::BadRequest(
            "session image must not be empty".to_owned(),
        ));
    }
    if !request.mounts.is_empty() {
        return Err(FabricHostError::BadRequest(
            "custom host mounts are not supported in P1".to_owned(),
        ));
    }
    if let Some(runtime_class) = &request.runtime_class {
        if runtime_class != "smolvm" {
            return Err(FabricHostError::BadRequest(format!(
                "unsupported runtime class '{runtime_class}'"
            )));
        }
    }
    Ok(())
}

#[cfg(feature = "smolvm-runtime")]
fn cpu_limit_to_vcpus(cpu_limit_millis: Option<u64>) -> u8 {
    let Some(cpu_limit_millis) = cpu_limit_millis else {
        return smolvm::VmResources::default().cpus;
    };
    let rounded = cpu_limit_millis.max(1).div_ceil(1000);
    rounded.clamp(1, u64::from(u8::MAX)) as u8
}

#[cfg(feature = "smolvm-runtime")]
fn memory_limit_to_mib(memory_limit_bytes: Option<u64>) -> u32 {
    let Some(memory_limit_bytes) = memory_limit_bytes else {
        return smolvm::VmResources::default().memory_mib;
    };
    let rounded = memory_limit_bytes.max(1).div_ceil(1024 * 1024);
    rounded.clamp(64, u64::from(u32::MAX)) as u32
}

#[cfg(feature = "smolvm-runtime")]
fn record_state_to_session_status(state: smolvm::RecordState) -> SessionStatus {
    match state {
        smolvm::RecordState::Created | smolvm::RecordState::Stopped => SessionStatus::Quiesced,
        smolvm::RecordState::Running => SessionStatus::Ready,
        smolvm::RecordState::Failed | smolvm::RecordState::Unreachable => SessionStatus::Error,
    }
}

#[cfg(feature = "smolvm-runtime")]
fn map_smolvm_error(error: smolvm::Error) -> FabricHostError {
    FabricHostError::Runtime(error.to_string())
}
