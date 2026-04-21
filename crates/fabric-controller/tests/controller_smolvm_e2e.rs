use std::{
    collections::BTreeMap,
    env, fs,
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, anyhow, bail};
use fabric_client::FabricControllerClient;
use fabric_protocol::{
    CloseSignal, ControllerExecRequest, ControllerSessionOpenRequest,
    ControllerSignalSessionRequest, ExecEvent, ExecEventKind, FabricBytes, FabricSandboxTarget,
    FabricSessionSignal, FabricSessionSignalKind, FabricSessionTarget, FsFileWriteRequest,
    FsPathQuery, NetworkMode, QuiesceSignal, RequestId, ResourceLimits, ResumeSignal, SessionId,
};
use futures_util::StreamExt;
use tokio::time::{Instant, sleep};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controller_drives_real_smolvm_host() -> anyhow::Result<()> {
    let Some(ctx) = ControllerSmolvmContext::start().await? else {
        return Ok(());
    };

    let image = env::var("FABRIC_SMOLVM_TEST_IMAGE").unwrap_or_else(|_| "alpine:latest".to_owned());
    let open_request = ControllerSessionOpenRequest {
        request_id: Some(RequestId(format!(
            "controller-smolvm-open-{}",
            unique_nonce()
        ))),
        target: FabricSessionTarget::Sandbox(FabricSandboxTarget {
            image,
            runtime_class: Some("smolvm".to_owned()),
            workdir: None,
            env: BTreeMap::new(),
            network_mode: NetworkMode::Egress,
            mounts: Vec::new(),
            resources: ResourceLimits::default(),
        }),
        ttl_ns: None,
        labels: BTreeMap::from([("test".to_owned(), "controller-smolvm".to_owned())]),
    };
    let open = ctx
        .client
        .open_session(&open_request)
        .await
        .context("open session through controller")?;
    let replayed_open = ctx
        .client
        .open_session(&open_request)
        .await
        .context("replay open session through controller")?;
    assert_eq!(replayed_open.session_id, open.session_id);
    assert_eq!(
        open.supported_signals,
        vec![
            FabricSessionSignalKind::Quiesce,
            FabricSessionSignalKind::Resume,
            FabricSessionSignalKind::Close,
        ]
    );
    let session_id = open.session_id.clone();

    let result = async {
        ctx.client
            .write_file(
                &session_id,
                &FsFileWriteRequest {
                    path: "controller.txt".to_owned(),
                    content: FabricBytes::Text("hello from controller\n".to_owned()),
                    create_parents: false,
                },
            )
            .await
            .context("write file through controller")?;
        let read = ctx
            .client
            .read_file(
                &session_id,
                &FsPathQuery {
                    path: "controller.txt".to_owned(),
                    offset_bytes: None,
                    max_bytes: None,
                },
            )
            .await
            .context("read file through controller")?;
        assert_eq!(read.content.as_text(), Some("hello from controller\n"));

        assert_controller_exec_streams(&ctx.client, &session_id).await?;

        let quiesced = signal(
            &ctx.controller_url,
            &session_id,
            FabricSessionSignal::Quiesce(QuiesceSignal {}),
        )
        .await
        .context("quiesce through controller")?;
        assert_eq!(
            quiesced.status,
            fabric_protocol::ControllerSessionStatus::Quiesced
        );

        let resumed = signal(
            &ctx.controller_url,
            &session_id,
            FabricSessionSignal::Resume(ResumeSignal {}),
        )
        .await
        .context("resume through controller")?;
        assert_eq!(
            resumed.status,
            fabric_protocol::ControllerSessionStatus::Ready
        );

        Ok(())
    }
    .await;

    let _ = signal(
        &ctx.controller_url,
        &session_id,
        FabricSessionSignal::Close(CloseSignal {}),
    )
    .await;

    result
}

async fn assert_controller_exec_streams(
    client: &FabricControllerClient,
    session_id: &SessionId,
) -> anyhow::Result<()> {
    let request = ControllerExecRequest {
        request_id: Some(RequestId(format!("controller-smolvm-exec-{}", unique_nonce()))),
        argv: vec![
            "sh".to_owned(),
            "-lc".to_owned(),
            "printf 'controller-one\\n'; sleep 2; printf 'controller-two\\n'; printf 'controller-err\\n' >&2"
                .to_owned(),
        ],
        cwd: None,
        env_patch: BTreeMap::new(),
        stdin: None,
        timeout_ns: Some(10_000_000_000),
    };
    let (first_events, first_stdout_after_start) =
        collect_controller_exec(client, session_id, &request)
            .await
            .context("start controller exec stream")?;
    assert_controller_exec_events(&first_events)?;
    let first_stdout_after_start =
        first_stdout_after_start.ok_or_else(|| anyhow!("controller exec produced no stdout"))?;
    assert!(
        first_stdout_after_start < Duration::from_millis(1500),
        "first stdout arrived after {first_stdout_after_start:?}; likely buffered until command exit"
    );

    let (second_events, _) = collect_controller_exec(client, session_id, &request)
        .await
        .context("replay controller exec stream")?;
    assert_eq!(
        serde_json::to_value(&first_events)?,
        serde_json::to_value(&second_events)?
    );

    Ok(())
}

async fn collect_controller_exec(
    client: &FabricControllerClient,
    session_id: &SessionId,
    request: &ControllerExecRequest,
) -> anyhow::Result<(Vec<fabric_protocol::ExecEvent>, Option<Duration>)> {
    let mut stream = client.exec_session_stream(session_id, request).await?;

    let mut started_at = None;
    let mut first_stdout_after_start = None;
    let mut events = Vec::new();

    while let Some(event) = stream.next().await {
        let event = event.context("read controller exec event")?;
        match event.kind {
            ExecEventKind::Started => started_at = Some(Instant::now()),
            ExecEventKind::Stdout => {
                if first_stdout_after_start.is_none()
                    && let Some(started_at) = started_at
                {
                    first_stdout_after_start = Some(started_at.elapsed());
                }
            }
            ExecEventKind::Stderr | ExecEventKind::Exit => {}
            ExecEventKind::Error => {
                bail!(
                    "controller exec error: {:?}",
                    event.message.clone().or(event_text(&event)?)
                )
            }
        }
        let done = event.kind == ExecEventKind::Exit;
        events.push(event);
        if done {
            break;
        }
    }

    Ok((events, first_stdout_after_start))
}

fn assert_controller_exec_events(events: &[fabric_protocol::ExecEvent]) -> anyhow::Result<()> {
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut exit_code = None;

    for event in events {
        match event.kind {
            ExecEventKind::Stdout => {
                if let Some(text) = event_text(event)? {
                    stdout.push_str(&text);
                }
            }
            ExecEventKind::Stderr => {
                if let Some(text) = event_text(event)? {
                    stderr.push_str(&text);
                }
            }
            ExecEventKind::Exit => {
                exit_code = event.exit_code;
            }
            ExecEventKind::Started => {}
            ExecEventKind::Error => {
                bail!(
                    "controller exec error: {:?}",
                    event.message.clone().or(event_text(event)?)
                )
            }
        }
    }

    assert_eq!(exit_code, Some(0));
    assert!(stdout.contains("controller-one\n"));
    assert!(stdout.contains("controller-two\n"));
    assert!(stderr.contains("controller-err\n"));

    Ok(())
}

async fn signal(
    controller_url: &str,
    session_id: &SessionId,
    signal: FabricSessionSignal,
) -> Result<fabric_protocol::ControllerSessionSummary, reqwest::Error> {
    reqwest::Client::new()
        .post(format!(
            "{}/v1/sessions/{}/signal",
            controller_url, session_id.0
        ))
        .json(&ControllerSignalSessionRequest {
            request_id: None,
            signal,
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
}

struct ControllerSmolvmContext {
    client: FabricControllerClient,
    controller_child: Child,
    host_child: Child,
    state_root: PathBuf,
    controller_state: PathBuf,
    controller_url: String,
}

impl ControllerSmolvmContext {
    async fn start() -> anyhow::Result<Option<Self>> {
        if env::var("FABRIC_SMOLVM_E2E").as_deref() != Ok("1") {
            eprintln!("skipping controller smolvm e2e: set FABRIC_SMOLVM_E2E=1 to enable");
            return Ok(None);
        }

        let Some(host_bin) = host_bin() else {
            eprintln!("skipping controller smolvm e2e: fabric-host binary not found");
            return Ok(None);
        };
        let Some(controller_bin) = controller_bin() else {
            eprintln!("skipping controller smolvm e2e: fabric-controller binary not found");
            return Ok(None);
        };

        let repo_root = repo_root();
        let rootfs = env::var_os("SMOLVM_AGENT_ROOTFS")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo_root.join("third_party/smolvm-release/agent-rootfs"));
        if !rootfs.exists() {
            eprintln!(
                "skipping controller smolvm e2e: smolvm agent rootfs not found at {}",
                rootfs.display()
            );
            return Ok(None);
        }

        let nonce = unique_nonce();
        let controller_addr = free_loopback_addr()?;
        let host_addr = free_loopback_addr()?;
        let controller_url = format!("http://{controller_addr}");
        let host_url = format!("http://{host_addr}");
        let controller_state =
            env::temp_dir().join(format!("fabric-controller-smolvm-e2e-{nonce}.sqlite"));
        let state_root = env::temp_dir().join(format!("fabric-host-controller-smolvm-e2e-{nonce}"));
        fs::create_dir_all(&state_root)?;

        let controller_child = Command::new(controller_bin)
            .arg("--bind")
            .arg(controller_addr.to_string())
            .arg("--db-path")
            .arg(&controller_state)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .context("spawn fabric-controller")?;
        let client = FabricControllerClient::new(controller_url.clone());
        wait_for_controller(&client).await?;

        let lib_dir = env::var_os("LIBKRUN_BUNDLE")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo_root.join("third_party/smolvm-release/lib"));
        let host_child = Command::new(host_bin)
            .arg("--bind")
            .arg(host_addr.to_string())
            .arg("--state-root")
            .arg(&state_root)
            .arg("--host-id")
            .arg(format!("controller-e2e-{nonce}"))
            .arg("--controller-url")
            .arg(&controller_url)
            .arg("--advertise-url")
            .arg(&host_url)
            .env("LIBKRUN_BUNDLE", &lib_dir)
            .env("DYLD_LIBRARY_PATH", &lib_dir)
            .env("SMOLVM_AGENT_ROOTFS", &rootfs)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .context("spawn fabric-host")?;

        wait_for_registered_host(&client).await?;

        Ok(Some(Self {
            client,
            controller_child,
            host_child,
            state_root,
            controller_state,
            controller_url,
        }))
    }
}

impl Drop for ControllerSmolvmContext {
    fn drop(&mut self) {
        let _ = self.host_child.kill();
        let _ = self.host_child.wait();
        let _ = self.controller_child.kill();
        let _ = self.controller_child.wait();
        let _ = fs::remove_dir_all(&self.state_root);
        let _ = fs::remove_file(&self.controller_state);
    }
}

async fn wait_for_controller(client: &FabricControllerClient) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match client.health().await {
            Ok(response) if response.ok => return Ok(()),
            Ok(_) | Err(_) if Instant::now() < deadline => sleep(Duration::from_millis(100)).await,
            Ok(response) => return Err(anyhow!("controller health check unhealthy: {response:?}")),
            Err(error) => return Err(error).context("wait for controller health"),
        }
    }
}

async fn wait_for_registered_host(client: &FabricControllerClient) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match client.list_hosts().await {
            Ok(hosts) if !hosts.hosts.is_empty() => return Ok(()),
            Ok(_) | Err(_) if Instant::now() < deadline => sleep(Duration::from_millis(100)).await,
            Ok(_) => return Err(anyhow!("host did not register with controller")),
            Err(error) => return Err(error).context("wait for host registration"),
        }
    }
}

fn host_bin() -> Option<PathBuf> {
    if let Some(path) = env::var_os("FABRIC_HOST_BIN").map(PathBuf::from)
        && path.exists()
    {
        return Some(path);
    }
    let path = repo_root().join("target/debug/fabric-host");
    path.exists().then_some(path)
}

fn controller_bin() -> Option<PathBuf> {
    if let Some(path) = option_env!("CARGO_BIN_EXE_fabric-controller").map(PathBuf::from)
        && path.exists()
    {
        return Some(path);
    }
    let path = repo_root().join("target/debug/fabric-controller");
    path.exists().then_some(path)
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("controller crate should live under crates/")
        .to_path_buf()
}

fn free_loopback_addr() -> anyhow::Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.local_addr().map_err(Into::into)
}

fn unique_nonce() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{nanos}", std::process::id())
}

fn event_text(event: &ExecEvent) -> anyhow::Result<Option<String>> {
    let Some(data) = &event.data else {
        return Ok(None);
    };
    let bytes = data.decode_bytes().map_err(anyhow::Error::msg)?;
    Ok(Some(String::from_utf8(bytes)?))
}
