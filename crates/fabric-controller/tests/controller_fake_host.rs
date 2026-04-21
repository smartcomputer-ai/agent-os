use std::{
    collections::BTreeMap,
    fs,
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use fabric_client::{FabricClientError, FabricControllerClient};
use fabric_controller::{FabricControllerConfig, FabricControllerService, FabricControllerState};
use fabric_protocol::{
    CloseSignal, ControllerExecRequest, ControllerSessionOpenRequest,
    ControllerSignalSessionRequest, ExecEvent, ExecEventKind, ExecId, ExecRequest, FabricBytes,
    FabricHostProvider, FabricSandboxTarget, FabricSessionSignal, FabricSessionSignalKind,
    FabricSessionTarget, FsDirEntry, FsEntryKind, FsExistsResponse, FsFileReadResponse,
    FsFileWriteRequest, FsListDirResponse, FsPathQuery, FsStatResponse, FsWriteResponse,
    HealthResponse, HostHeartbeatRequest, HostId, HostInventoryResponse, HostInventorySession,
    HostRegisterRequest, NetworkMode, ProviderCapacity, QuiesceSignal, RequestId, ResourceLimits,
    ResumeSignal, SessionId, SessionLabelsPatchRequest, SessionOpenRequest, SessionOpenResponse,
    SessionSignal, SessionStatus, SessionStatusResponse, SignalSessionRequest, SmolvmProviderInfo,
    TerminateRuntimeSignal,
};
use futures_util::StreamExt;
use tokio::task::JoinHandle;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controller_schedules_proxies_replays_and_reconciles_with_fake_host() -> anyhow::Result<()>
{
    let fake_host = FakeHost::start().await?;
    let controller = TestController::start().await?;
    let client = FabricControllerClient::new(controller.base_url.clone());

    client
        .register_host(&HostRegisterRequest {
            host_id: HostId("fake-a".to_owned()),
            endpoint: fake_host.base_url.clone(),
            providers: vec![smolvm_provider(0)],
            labels: BTreeMap::from([("pool".to_owned(), "test".to_owned())]),
        })
        .await
        .context("register fake host")?;

    let open_request = ControllerSessionOpenRequest {
        request_id: Some(RequestId("open-1".to_owned())),
        target: FabricSessionTarget::Sandbox(FabricSandboxTarget {
            image: "alpine:latest".to_owned(),
            runtime_class: Some("smolvm".to_owned()),
            workdir: None,
            env: BTreeMap::new(),
            network_mode: NetworkMode::Egress,
            mounts: Vec::new(),
            resources: ResourceLimits::default(),
        }),
        ttl_ns: None,
        labels: BTreeMap::from([
            ("world".to_owned(), "dev".to_owned()),
            ("task".to_owned(), "fake".to_owned()),
        ]),
    };
    let opened = client
        .open_session(&open_request)
        .await
        .context("open controller session")?;
    assert_eq!(
        opened.status,
        fabric_protocol::ControllerSessionStatus::Ready
    );
    assert_eq!(
        opened.supported_signals,
        vec![
            FabricSessionSignalKind::Quiesce,
            FabricSessionSignalKind::Resume,
            FabricSessionSignalKind::Close,
        ]
    );
    assert_eq!(fake_host.open_count(), 1);

    let replayed = client
        .open_session(&open_request)
        .await
        .context("replay controller session open")?;
    assert_eq!(replayed.session_id, opened.session_id);
    assert_eq!(fake_host.open_count(), 1);

    let mut conflicting_open = open_request.clone();
    conflicting_open.target = FabricSessionTarget::Sandbox(FabricSandboxTarget {
        image: "ubuntu:latest".to_owned(),
        runtime_class: Some("smolvm".to_owned()),
        workdir: None,
        env: BTreeMap::new(),
        network_mode: NetworkMode::Egress,
        mounts: Vec::new(),
        resources: ResourceLimits::default(),
    });
    assert_server_code(
        client.open_session(&conflicting_open).await,
        "idempotency_key_conflict",
    );

    let listed = client
        .list_sessions(&[
            ("world".to_owned(), "dev".to_owned()),
            ("task".to_owned(), "fake".to_owned()),
        ])
        .await
        .context("list controller sessions by label")?;
    assert_eq!(listed.sessions.len(), 1);
    assert_eq!(listed.sessions[0].session_id, opened.session_id);

    let labels = client
        .patch_session_labels(
            &opened.session_id,
            &SessionLabelsPatchRequest {
                set: BTreeMap::from([("phase".to_owned(), "patched".to_owned())]),
                remove: vec!["task".to_owned()],
            },
        )
        .await
        .context("patch session labels")?;
    assert_eq!(
        labels.labels.get("phase").map(String::as_str),
        Some("patched")
    );
    assert!(!labels.labels.contains_key("task"));

    client
        .write_file(
            &opened.session_id,
            &FsFileWriteRequest {
                path: "hello.txt".to_owned(),
                content: FabricBytes::Text("hello fake fs\n".to_owned()),
                create_parents: false,
            },
        )
        .await
        .context("write file through controller")?;
    let read = client
        .read_file(
            &opened.session_id,
            &FsPathQuery {
                path: "hello.txt".to_owned(),
                offset_bytes: None,
                max_bytes: None,
            },
        )
        .await
        .context("read file through controller")?;
    assert_eq!(read.content.as_text(), Some("hello fake fs\n"));
    let exists = client
        .exists(
            &opened.session_id,
            &FsPathQuery {
                path: "hello.txt".to_owned(),
                offset_bytes: None,
                max_bytes: None,
            },
        )
        .await
        .context("exists through controller")?;
    assert!(exists.exists);
    let stat = client
        .stat(
            &opened.session_id,
            &FsPathQuery {
                path: "hello.txt".to_owned(),
                offset_bytes: None,
                max_bytes: None,
            },
        )
        .await
        .context("stat through controller")?;
    assert_eq!(stat.kind, FsEntryKind::File);
    assert_eq!(stat.size_bytes, "hello fake fs\n".len() as u64);
    let list = client
        .list_dir(
            &opened.session_id,
            &FsPathQuery {
                path: ".".to_owned(),
                offset_bytes: None,
                max_bytes: None,
            },
        )
        .await
        .context("list dir through controller")?;
    assert!(list.entries.iter().any(|entry| entry.name == "hello.txt"));

    let exec_request = ControllerExecRequest {
        request_id: Some(RequestId("exec-1".to_owned())),
        argv: vec!["sh".to_owned(), "-lc".to_owned(), "ignored".to_owned()],
        cwd: None,
        env_patch: BTreeMap::new(),
        stdin: None,
        timeout_ns: Some(5_000_000_000),
    };
    let first_exec = collect_exec(
        client
            .exec_session_stream(&opened.session_id, &exec_request)
            .await
            .context("exec through controller")?,
    )
    .await?;
    let second_exec = collect_exec(
        client
            .exec_session_stream(&opened.session_id, &exec_request)
            .await
            .context("replay exec through controller")?,
    )
    .await?;
    assert_eq!(
        serde_json::to_value(&first_exec)?,
        serde_json::to_value(&second_exec)?
    );
    assert_eq!(fake_host.exec_count(), 1);
    assert!(first_exec.iter().any(|event| {
        event.kind == ExecEventKind::Stdout
            && event.data.as_ref().and_then(FabricBytes::as_text) == Some("fake-stdout\n")
    }));

    let binary_stdin = vec![0, 159, 255, b'\n'];
    let binary_exec_request = ControllerExecRequest {
        request_id: Some(RequestId("exec-binary-1".to_owned())),
        argv: vec!["cat".to_owned()],
        cwd: None,
        env_patch: BTreeMap::new(),
        stdin: Some(FabricBytes::from_bytes_base64(&binary_stdin).into()),
        timeout_ns: Some(5_000_000_000),
    };
    let binary_first_exec = collect_exec(
        client
            .exec_session_stream(&opened.session_id, &binary_exec_request)
            .await
            .context("binary exec through controller")?,
    )
    .await?;
    let binary_second_exec = collect_exec(
        client
            .exec_session_stream(&opened.session_id, &binary_exec_request)
            .await
            .context("replay binary exec through controller")?,
    )
    .await?;
    assert_eq!(
        serde_json::to_value(&binary_first_exec)?,
        serde_json::to_value(&binary_second_exec)?
    );
    assert_eq!(fake_host.exec_count(), 2);
    assert_eq!(stdout_bytes(&binary_first_exec)?, binary_stdin);

    let quiesced = post_signal(
        &controller.base_url,
        &opened.session_id,
        FabricSessionSignal::Quiesce(QuiesceSignal {}),
    )
    .await?;
    assert_eq!(
        quiesced.status,
        fabric_protocol::ControllerSessionStatus::Quiesced
    );
    let resumed = post_signal(
        &controller.base_url,
        &opened.session_id,
        FabricSessionSignal::Resume(ResumeSignal {}),
    )
    .await?;
    assert_eq!(
        resumed.status,
        fabric_protocol::ControllerSessionStatus::Ready
    );
    let unsupported = reqwest::Client::new()
        .post(format!(
            "{}/v1/sessions/{}/signal",
            controller.base_url, opened.session_id.0
        ))
        .json(&ControllerSignalSessionRequest {
            request_id: None,
            signal: FabricSessionSignal::TerminateRuntime(TerminateRuntimeSignal {}),
        })
        .send()
        .await?;
    assert_eq!(unsupported.status(), StatusCode::UNPROCESSABLE_ENTITY);

    client
        .heartbeat_host(
            &HostId("fake-a".to_owned()),
            &HostHeartbeatRequest {
                host_id: HostId("fake-a".to_owned()),
                endpoint: Some(fake_host.base_url.clone()),
                providers: vec![smolvm_provider(1)],
                inventory: Some(HostInventoryResponse {
                    host_id: HostId("fake-a".to_owned()),
                    sessions: vec![HostInventorySession {
                        session_id: opened.session_id.clone(),
                        status: SessionStatus::Quiesced,
                        machine_name: Some("fake-machine".to_owned()),
                        image: Some("alpine:latest".to_owned()),
                        workspace_path: None,
                        workdir: Some("/workspace".to_owned()),
                        network_mode: Some(NetworkMode::Egress),
                        runtime_present: true,
                        workspace_present: true,
                        marker_present: true,
                        created_at_ns: Some(now_ns()),
                        expires_at_ns: None,
                        labels: BTreeMap::new(),
                    }],
                }),
                labels: BTreeMap::new(),
            },
        )
        .await
        .context("heartbeat inventory")?;
    let reconciled = client
        .session(&opened.session_id)
        .await
        .context("read reconciled session")?;
    assert_eq!(
        reconciled.status,
        fabric_protocol::ControllerSessionStatus::Quiesced
    );
    assert!(
        !reconciled
            .supported_signals
            .contains(&FabricSessionSignalKind::TerminateRuntime)
    );

    let db_path = controller.into_db_path();
    let controller = TestController::start_with_db(db_path, true).await?;
    let client = FabricControllerClient::new(controller.base_url.clone());
    let reloaded = client
        .session(&opened.session_id)
        .await
        .context("read session after controller restart")?;
    assert_eq!(
        reloaded.status,
        fabric_protocol::ControllerSessionStatus::Quiesced
    );

    let closed = post_signal(
        &controller.base_url,
        &opened.session_id,
        FabricSessionSignal::Close(CloseSignal {}),
    )
    .await?;
    assert_eq!(
        closed.status,
        fabric_protocol::ControllerSessionStatus::Closed
    );
    assert!(closed.closed_at_ns.is_some());

    Ok(())
}

struct TestController {
    base_url: String,
    task: JoinHandle<()>,
    db_path: PathBuf,
    cleanup_db: bool,
}

impl TestController {
    async fn start() -> anyhow::Result<Self> {
        let db_path =
            std::env::temp_dir().join(format!("fabric-controller-test-{}.sqlite", now_ns()));
        Self::start_with_db(db_path, true).await
    }

    async fn start_with_db(db_path: PathBuf, cleanup_db: bool) -> anyhow::Result<Self> {
        let addr = free_loopback_addr()?;
        let config = FabricControllerConfig {
            bind_addr: addr,
            db_path: db_path.clone(),
            ..Default::default()
        };
        let state = FabricControllerState::open(&db_path)?;
        let service = Arc::new(FabricControllerService::new(config, state));
        let router = fabric_controller::http::router(service);
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok(Self {
            base_url: format!("http://{addr}"),
            task,
            db_path,
            cleanup_db,
        })
    }

    fn into_db_path(mut self) -> PathBuf {
        self.cleanup_db = false;
        self.task.abort();
        self.db_path.clone()
    }
}

impl Drop for TestController {
    fn drop(&mut self) {
        self.task.abort();
        if self.cleanup_db {
            let _ = fs::remove_file(&self.db_path);
        }
    }
}

#[derive(Default)]
struct FakeHostState {
    sessions: Mutex<BTreeMap<SessionId, SessionStatus>>,
    files: Mutex<BTreeMap<(SessionId, String), Vec<u8>>>,
    open_count: AtomicU64,
    exec_count: AtomicU64,
}

#[derive(Clone)]
struct FakeHost {
    base_url: String,
    state: Arc<FakeHostState>,
    _task: Arc<JoinHandle<()>>,
}

impl FakeHost {
    async fn start() -> anyhow::Result<Self> {
        let state = Arc::new(FakeHostState::default());
        let router = Router::new()
            .route("/healthz", get(fake_healthz))
            .route("/v1/sessions", post(fake_open_session))
            .route("/v1/sessions/{session_id}", get(fake_session_status))
            .route("/v1/sessions/{session_id}/exec", post(fake_exec))
            .route("/v1/sessions/{session_id}/signal", post(fake_signal))
            .route(
                "/v1/sessions/{session_id}/fs/file",
                get(fake_read_file).put(fake_write_file),
            )
            .route("/v1/sessions/{session_id}/fs/exists", get(fake_exists))
            .route("/v1/sessions/{session_id}/fs/stat", get(fake_stat))
            .route("/v1/sessions/{session_id}/fs/list_dir", get(fake_list_dir))
            .with_state(state.clone());

        let addr = free_loopback_addr()?;
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let task = Arc::new(tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        }));

        Ok(Self {
            base_url: format!("http://{addr}"),
            state,
            _task: task,
        })
    }

    fn open_count(&self) -> u64 {
        self.state.open_count.load(Ordering::SeqCst)
    }

    fn exec_count(&self) -> u64 {
        self.state.exec_count.load(Ordering::SeqCst)
    }
}

async fn fake_healthz() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        service: "fake-host".to_owned(),
    })
}

async fn fake_open_session(
    State(state): State<Arc<FakeHostState>>,
    Json(request): Json<SessionOpenRequest>,
) -> Json<SessionOpenResponse> {
    state.open_count.fetch_add(1, Ordering::SeqCst);
    let session_id = request
        .session_id
        .unwrap_or_else(|| SessionId(format!("fake-{}", now_ns())));
    state
        .sessions
        .lock()
        .unwrap()
        .insert(session_id.clone(), SessionStatus::Ready);
    Json(SessionOpenResponse {
        session_id,
        status: SessionStatus::Ready,
        workdir: "/workspace".to_owned(),
        host_id: Some(HostId("fake-a".to_owned())),
    })
}

async fn fake_session_status(
    State(state): State<Arc<FakeHostState>>,
    Path(session_id): Path<String>,
) -> Json<SessionStatusResponse> {
    let session_id = SessionId(session_id);
    let status = state
        .sessions
        .lock()
        .unwrap()
        .get(&session_id)
        .copied()
        .unwrap_or(SessionStatus::Error);
    Json(SessionStatusResponse { session_id, status })
}

async fn fake_exec(
    State(state): State<Arc<FakeHostState>>,
    Path(_session_id): Path<String>,
    Json(request): Json<ExecRequest>,
) -> impl IntoResponse {
    let count = state.exec_count.fetch_add(1, Ordering::SeqCst);
    let exec_id = ExecId(format!("fake-exec-{count}"));
    let stdout = request
        .stdin
        .as_ref()
        .map(FabricBytes::decode_bytes)
        .transpose()
        .unwrap()
        .unwrap_or_else(|| b"fake-stdout\n".to_vec());
    let events = vec![
        ExecEvent {
            exec_id: exec_id.clone(),
            seq: 0,
            kind: ExecEventKind::Started,
            data: None,
            exit_code: None,
            message: None,
        },
        ExecEvent {
            exec_id: exec_id.clone(),
            seq: 1,
            kind: ExecEventKind::Stdout,
            data: Some(FabricBytes::from_bytes_auto(stdout)),
            exit_code: None,
            message: None,
        },
        ExecEvent {
            exec_id: exec_id.clone(),
            seq: 2,
            kind: ExecEventKind::Stderr,
            data: Some(FabricBytes::Text("fake-stderr\n".to_owned())),
            exit_code: None,
            message: None,
        },
        ExecEvent {
            exec_id,
            seq: 3,
            kind: ExecEventKind::Exit,
            data: None,
            exit_code: Some(0),
            message: None,
        },
    ];
    let mut body = Vec::new();
    for event in events {
        body.extend_from_slice(&serde_json::to_vec(&event).unwrap());
        body.push(b'\n');
    }

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        Body::from(Bytes::from(body)),
    )
}

async fn fake_signal(
    State(state): State<Arc<FakeHostState>>,
    Path(session_id): Path<String>,
    Json(request): Json<SignalSessionRequest>,
) -> Json<SessionStatusResponse> {
    let session_id = SessionId(session_id);
    let status = match request.action {
        SessionSignal::Quiesce | SessionSignal::Terminate => SessionStatus::Quiesced,
        SessionSignal::Resume => SessionStatus::Ready,
        SessionSignal::Close => SessionStatus::Closed,
    };
    state
        .sessions
        .lock()
        .unwrap()
        .insert(session_id.clone(), status);
    Json(SessionStatusResponse { session_id, status })
}

async fn fake_write_file(
    State(state): State<Arc<FakeHostState>>,
    Path(session_id): Path<String>,
    Json(request): Json<FsFileWriteRequest>,
) -> Json<FsWriteResponse> {
    let content = request.content.decode_bytes().unwrap();
    let bytes_written = content.len() as u64;
    state
        .files
        .lock()
        .unwrap()
        .insert((SessionId(session_id), request.path.clone()), content);
    Json(FsWriteResponse {
        path: format!("/workspace/{}", request.path),
        bytes_written,
    })
}

async fn fake_read_file(
    State(state): State<Arc<FakeHostState>>,
    Path(session_id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Json<FsFileReadResponse> {
    let bytes = state
        .files
        .lock()
        .unwrap()
        .get(&(SessionId(session_id), query.path.clone()))
        .cloned()
        .unwrap_or_default();
    Json(FsFileReadResponse {
        path: format!("/workspace/{}", query.path),
        offset_bytes: 0,
        bytes_read: bytes.len() as u64,
        size_bytes: bytes.len() as u64,
        truncated: false,
        content: FabricBytes::from_bytes_auto(bytes),
        mtime_ns: None,
    })
}

async fn fake_exists(
    State(state): State<Arc<FakeHostState>>,
    Path(session_id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Json<FsExistsResponse> {
    let exists = state
        .files
        .lock()
        .unwrap()
        .contains_key(&(SessionId(session_id), query.path.clone()));
    Json(FsExistsResponse {
        path: format!("/workspace/{}", query.path),
        exists,
    })
}

async fn fake_stat(
    State(state): State<Arc<FakeHostState>>,
    Path(session_id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Json<FsStatResponse> {
    let size_bytes = state
        .files
        .lock()
        .unwrap()
        .get(&(SessionId(session_id), query.path.clone()))
        .map(|bytes| bytes.len() as u64)
        .unwrap_or_default();
    Json(FsStatResponse {
        path: format!("/workspace/{}", query.path),
        kind: FsEntryKind::File,
        size_bytes,
        readonly: false,
        mtime_ns: None,
    })
}

async fn fake_list_dir(
    State(state): State<Arc<FakeHostState>>,
    Path(session_id): Path<String>,
    Query(_query): Query<FsPathQuery>,
) -> Json<FsListDirResponse> {
    let session_id = SessionId(session_id);
    let entries = state
        .files
        .lock()
        .unwrap()
        .iter()
        .filter_map(|((stored_session_id, path), text)| {
            (stored_session_id == &session_id).then_some(FsDirEntry {
                name: path.clone(),
                path: format!("/workspace/{path}"),
                kind: FsEntryKind::File,
                size_bytes: text.len() as u64,
                readonly: false,
            })
        })
        .collect();
    Json(FsListDirResponse {
        path: "/workspace".to_owned(),
        entries,
    })
}

async fn collect_exec(
    mut stream: fabric_client::ExecEventClientStream,
) -> anyhow::Result<Vec<ExecEvent>> {
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event?);
    }
    Ok(events)
}

fn stdout_bytes(events: &[ExecEvent]) -> anyhow::Result<Vec<u8>> {
    let mut output = Vec::new();
    for event in events {
        if event.kind == ExecEventKind::Stdout {
            if let Some(data) = &event.data {
                output.extend(data.decode_bytes().map_err(anyhow::Error::msg)?);
            }
        }
    }
    Ok(output)
}

async fn post_signal(
    base_url: &str,
    session_id: &SessionId,
    signal: FabricSessionSignal,
) -> anyhow::Result<fabric_protocol::ControllerSessionSummary> {
    Ok(reqwest::Client::new()
        .post(format!("{base_url}/v1/sessions/{}/signal", session_id.0))
        .json(&ControllerSignalSessionRequest {
            request_id: None,
            signal,
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

fn smolvm_provider(active_sessions: u64) -> FabricHostProvider {
    FabricHostProvider::Smolvm(SmolvmProviderInfo {
        runtime_version: None,
        supported_runtime_classes: vec!["smolvm".to_owned()],
        allowed_images: vec!["*".to_owned()],
        allowed_network_modes: vec![NetworkMode::Disabled, NetworkMode::Egress],
        resource_defaults: ResourceLimits::default(),
        resource_max: ResourceLimits::default(),
        capacity: ProviderCapacity {
            max_sessions: Some(10),
            active_sessions,
            max_concurrent_execs: None,
            active_execs: 0,
        },
    })
}

fn assert_server_code<T>(result: Result<T, FabricClientError>, expected: &str) {
    match result {
        Err(FabricClientError::Server { code, .. }) => assert_eq!(code, expected),
        Ok(_) => panic!("expected server error {expected}, got success"),
        Err(error) => panic!("expected server error {expected}, got {error}"),
    }
}

fn free_loopback_addr() -> anyhow::Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.local_addr().map_err(Into::into)
}

fn now_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
