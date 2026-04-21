use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, anyhow, bail};
use fabric_protocol::{
    ExecEvent, ExecEventKind, ExecRequest, FabricBytes, FsFileWriteRequest, FsPathQuery,
    NetworkMode, ResourceLimits, SessionId, SessionOpenRequest, SessionSignal, SessionStatus,
    SignalSessionRequest,
};
use futures_util::StreamExt;
use tokio::time::Instant;

mod support;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smolvm_session_lifecycle_smoke() -> anyhow::Result<()> {
    let Some(ctx) = support::E2eContext::start().await? else {
        return Ok(());
    };

    let image =
        std::env::var("FABRIC_SMOLVM_TEST_IMAGE").unwrap_or_else(|_| "alpine:latest".to_owned());
    let session_id = SessionId(format!("smolvm-e2e-{}", std::process::id()));
    let mut session_opened = false;

    let result = async {
        let open = ctx
            .client
            .open_session(&SessionOpenRequest {
                session_id: Some(session_id.clone()),
                image,
                runtime_class: Some("smolvm".to_owned()),
                workdir: None,
                env: BTreeMap::new(),
                network_mode: NetworkMode::Egress,
                mounts: Vec::new(),
                resources: ResourceLimits::default(),
                ttl_secs: None,
                labels: BTreeMap::new(),
            })
            .await
            .context("open smolvm session")?;
        session_opened = true;
        assert_eq!(open.status, SessionStatus::Ready);

        ctx.client
            .write_file(
                &session_id,
                &FsFileWriteRequest {
                    path: "hello.txt".to_owned(),
                    content: FabricBytes::Text("hello from fabric\n".to_owned()),
                    create_parents: false,
                },
            )
            .await
            .context("write workspace file")?;

        let read = ctx
            .client
            .read_file(
                &session_id,
                &FsPathQuery {
                    path: "hello.txt".to_owned(),
                    offset_bytes: None,
                    max_bytes: None,
                },
            )
            .await
            .context("read workspace file")?;
        assert_eq!(read.content.as_text(), Some("hello from fabric\n"));

        assert_live_exec_streaming(&ctx.client, &session_id).await?;
        assert_concurrent_execs(&ctx.client, &session_id).await?;

        ctx.client
            .signal_session(
                &session_id,
                &SignalSessionRequest {
                    action: SessionSignal::Quiesce,
                },
            )
            .await
            .context("quiesce session")?;
        let status = ctx
            .client
            .session_status(&session_id)
            .await
            .context("status after quiesce")?;
        assert_eq!(status.status, SessionStatus::Quiesced);

        ctx.client
            .signal_session(
                &session_id,
                &SignalSessionRequest {
                    action: SessionSignal::Resume,
                },
            )
            .await
            .context("resume session")?;
        let status = ctx
            .client
            .session_status(&session_id)
            .await
            .context("status after resume")?;
        assert_eq!(status.status, SessionStatus::Ready);

        let inventory = ctx
            .client
            .inventory()
            .await
            .context("query host inventory")?;
        let session = inventory
            .sessions
            .iter()
            .find(|entry| entry.session_id == session_id)
            .ok_or_else(|| anyhow!("session missing from inventory"))?;
        assert!(session.runtime_present);
        assert!(session.workspace_present);
        assert!(session.marker_present);

        Ok(())
    }
    .await;

    if session_opened {
        let _ = ctx
            .client
            .signal_session(
                &session_id,
                &SignalSessionRequest {
                    action: SessionSignal::Close,
                },
            )
            .await;
    }

    result
}

async fn assert_live_exec_streaming(
    client: &fabric_client::FabricHostClient,
    session_id: &SessionId,
) -> anyhow::Result<()> {
    let mut stream = client
        .exec_session_stream(&ExecRequest {
            session_id: session_id.clone(),
            argv: vec![
                "sh".to_owned(),
                "-lc".to_owned(),
                "printf 'stream-one\\n'; sleep 2; printf 'stream-two\\n'; printf 'stream-err\\n' >&2"
                    .to_owned(),
            ],
            cwd: None,
            env: BTreeMap::new(),
            stdin: None,
            timeout_secs: Some(10),
        })
        .await
        .context("start streaming exec")?;

    let mut started_at = None;
    let mut first_stdout_after_start = None;
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut exit_code = None;

    while let Some(event) = stream.next().await {
        let event = event.context("read streaming exec event")?;
        match event.kind {
            ExecEventKind::Started => started_at = Some(Instant::now()),
            ExecEventKind::Stdout => {
                if first_stdout_after_start.is_none()
                    && let Some(started_at) = started_at
                {
                    first_stdout_after_start = Some(started_at.elapsed());
                }
                if let Some(text) = event_text(&event)? {
                    stdout.push_str(&text);
                }
            }
            ExecEventKind::Stderr => {
                if let Some(text) = event_text(&event)? {
                    stderr.push_str(&text);
                }
            }
            ExecEventKind::Exit => {
                exit_code = event.exit_code;
                break;
            }
            ExecEventKind::Error => {
                bail!(
                    "streaming exec error: {:?}",
                    event.message.clone().or(event_text(&event)?)
                );
            }
        }
    }

    assert_eq!(exit_code, Some(0));
    assert!(stdout.contains("stream-one\n"));
    assert!(stdout.contains("stream-two\n"));
    assert!(stderr.contains("stream-err\n"));
    let first_stdout_after_start =
        first_stdout_after_start.ok_or_else(|| anyhow!("streaming exec produced no stdout"))?;
    assert!(
        first_stdout_after_start < Duration::from_millis(1500),
        "first stdout arrived after {first_stdout_after_start:?}; likely buffered until command exit"
    );

    Ok(())
}

async fn assert_concurrent_execs(
    client: &fabric_client::FabricHostClient,
    session_id: &SessionId,
) -> anyhow::Result<()> {
    let left = exec_collecting_stdout(client.clone(), session_id.clone(), "left");
    let right = exec_collecting_stdout(client.clone(), session_id.clone(), "right");
    let (left, right) = tokio::join!(left, right);

    assert_eq!(left.context("left concurrent exec")?, "left\n");
    assert_eq!(right.context("right concurrent exec")?, "right\n");
    Ok(())
}

async fn exec_collecting_stdout(
    client: fabric_client::FabricHostClient,
    session_id: SessionId,
    label: &'static str,
) -> anyhow::Result<String> {
    let events = client
        .exec_session(&ExecRequest {
            session_id,
            argv: vec![
                "sh".to_owned(),
                "-lc".to_owned(),
                format!("sleep 1; printf '{label}\\n'"),
            ],
            cwd: None,
            env: BTreeMap::new(),
            stdin: None,
            timeout_secs: Some(10),
        })
        .await
        .with_context(|| format!("exec {label}"))?;

    let mut stdout = String::new();
    let mut exit_code = None;
    for event in events {
        match event.kind {
            ExecEventKind::Stdout => {
                if let Some(text) = event_text(&event)? {
                    stdout.push_str(&text);
                }
            }
            ExecEventKind::Exit => exit_code = event.exit_code,
            ExecEventKind::Error => bail!(
                "exec {label} error: {:?}",
                event.message.clone().or(event_text(&event)?)
            ),
            ExecEventKind::Started | ExecEventKind::Stderr => {}
        }
    }

    assert_eq!(exit_code, Some(0));
    Ok(stdout)
}

fn event_text(event: &ExecEvent) -> anyhow::Result<Option<String>> {
    let Some(data) = &event.data else {
        return Ok(None);
    };
    let bytes = data.decode_bytes().map_err(anyhow::Error::msg)?;
    Ok(Some(String::from_utf8(bytes)?))
}
