use std::collections::{BTreeMap, HashMap};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effects::builtins::{
    HostBlobOutput, HostExecParams, HostExecReceipt, HostInlineBytes, HostInlineText, HostOutput,
    HostSessionOpenParams, HostSessionOpenReceipt, HostSessionSignalParams,
    HostSessionSignalReceipt,
};
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use aos_store::Store;
use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::Mutex;

use super::traits::AsyncEffectAdapter;

const INLINE_OUTPUT_LIMIT_BYTES: usize = 16 * 1024;
const OUTPUT_PREVIEW_BYTES: usize = 512;

#[derive(Default)]
struct HostState {
    next_session_id: u64,
    sessions: HashMap<String, SessionRecord>,
}

#[derive(Clone)]
struct SessionRecord {
    workdir: Option<String>,
    env: BTreeMap<String, String>,
    expires_at_ns: Option<u64>,
    closed: bool,
    ended_at_ns: Option<u64>,
    last_exit_code: Option<i32>,
}

pub struct HostSessionOpenAdapter {
    state: Arc<Mutex<HostState>>,
}

pub struct HostExecAdapter<S: Store> {
    state: Arc<Mutex<HostState>>,
    store: Arc<S>,
}

pub struct HostSessionSignalAdapter {
    state: Arc<Mutex<HostState>>,
}

pub fn make_host_adapters<S: Store + Send + Sync + 'static>(
    store: Arc<S>,
) -> (
    HostSessionOpenAdapter,
    HostExecAdapter<S>,
    HostSessionSignalAdapter,
) {
    let state = Arc::new(Mutex::new(HostState::default()));
    (
        HostSessionOpenAdapter {
            state: state.clone(),
        },
        HostExecAdapter {
            state: state.clone(),
            store,
        },
        HostSessionSignalAdapter { state },
    )
}

#[async_trait]
impl AsyncEffectAdapter for HostSessionOpenAdapter {
    fn kind(&self) -> &str {
        EffectKind::HOST_SESSION_OPEN
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let started_at_ns = now_wallclock_ns();
        let params: HostSessionOpenParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => {
                let payload = HostSessionOpenReceipt {
                    session_id: String::new(),
                    status: "error".into(),
                    started_at_ns,
                    expires_at_ns: None,
                    error_code: Some("invalid_params".into()),
                    error_message: Some(err.to_string()),
                };
                return Ok(build_receipt(
                    intent,
                    "host.session.open",
                    ReceiptStatus::Error,
                    &payload,
                )?);
            }
        };

        let Some(target_local) = params.target.local else {
            let payload = HostSessionOpenReceipt {
                session_id: String::new(),
                status: "error".into(),
                started_at_ns,
                expires_at_ns: None,
                error_code: Some("unsupported_target".into()),
                error_message: Some("target.local is required".into()),
            };
            return Ok(build_receipt(
                intent,
                "host.session.open",
                ReceiptStatus::Error,
                &payload,
            )?);
        };

        let mut state = self.state.lock().await;
        state.next_session_id += 1;
        let session_id = format!("sess-{}", state.next_session_id);
        let expires_at_ns = params
            .session_ttl_ns
            .map(|ttl| started_at_ns.saturating_add(ttl));

        state.sessions.insert(
            session_id.clone(),
            SessionRecord {
                workdir: target_local.workdir,
                env: target_local.env.unwrap_or_default().into_iter().collect(),
                expires_at_ns,
                closed: false,
                ended_at_ns: None,
                last_exit_code: None,
            },
        );

        let payload = HostSessionOpenReceipt {
            session_id,
            status: "ready".into(),
            started_at_ns,
            expires_at_ns,
            error_code: None,
            error_message: None,
        };

        build_receipt(intent, "host.session.open", ReceiptStatus::Ok, &payload)
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for HostExecAdapter<S> {
    fn kind(&self) -> &str {
        EffectKind::HOST_EXEC
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let started_at_ns = now_wallclock_ns();
        let params: HostExecParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => {
                let payload = HostExecReceipt {
                    exit_code: -1,
                    status: "error".into(),
                    stdout: None,
                    stderr: None,
                    started_at_ns,
                    ended_at_ns: now_wallclock_ns(),
                    error_code: Some("invalid_params".into()),
                    error_message: Some(err.to_string()),
                };
                return Ok(build_receipt(
                    intent,
                    "host.exec",
                    ReceiptStatus::Error,
                    &payload,
                )?);
            }
        };

        let session = {
            let mut state = self.state.lock().await;
            let Some(session) = state.sessions.get_mut(&params.session_id) else {
                let payload = HostExecReceipt {
                    exit_code: -1,
                    status: "error".into(),
                    stdout: None,
                    stderr: None,
                    started_at_ns,
                    ended_at_ns: now_wallclock_ns(),
                    error_code: Some("session_not_found".into()),
                    error_message: Some(format!("session '{}' not found", params.session_id)),
                };
                return Ok(build_receipt(
                    intent,
                    "host.exec",
                    ReceiptStatus::Error,
                    &payload,
                )?);
            };
            if session.closed {
                let payload = HostExecReceipt {
                    exit_code: -1,
                    status: "error".into(),
                    stdout: None,
                    stderr: None,
                    started_at_ns,
                    ended_at_ns: now_wallclock_ns(),
                    error_code: Some("session_closed".into()),
                    error_message: Some(format!("session '{}' is closed", params.session_id)),
                };
                return Ok(build_receipt(
                    intent,
                    "host.exec",
                    ReceiptStatus::Error,
                    &payload,
                )?);
            }
            if let Some(expires_at_ns) = session.expires_at_ns {
                let now = now_wallclock_ns();
                if now > expires_at_ns {
                    session.closed = true;
                    session.ended_at_ns = Some(now);
                    let payload = HostExecReceipt {
                        exit_code: -1,
                        status: "error".into(),
                        stdout: None,
                        stderr: None,
                        started_at_ns,
                        ended_at_ns: now,
                        error_code: Some("session_expired".into()),
                        error_message: Some(format!(
                            "session '{}' expired at {}",
                            params.session_id, expires_at_ns
                        )),
                    };
                    return Ok(build_receipt(
                        intent,
                        "host.exec",
                        ReceiptStatus::Error,
                        &payload,
                    )?);
                }
            }
            session.clone()
        };

        let mode = params.output_mode.as_deref().unwrap_or("auto");
        if mode != "auto" && mode != "require_inline" {
            let payload = HostExecReceipt {
                exit_code: -1,
                status: "error".into(),
                stdout: None,
                stderr: None,
                started_at_ns,
                ended_at_ns: now_wallclock_ns(),
                error_code: Some("invalid_output_mode".into()),
                error_message: Some(format!("unsupported output_mode '{}'", mode)),
            };
            return Ok(build_receipt(
                intent,
                "host.exec",
                ReceiptStatus::Error,
                &payload,
            )?);
        }

        let stdin_bytes = match params.stdin_ref.as_ref() {
            Some(stdin_ref) => {
                let hash = match Hash::from_hex_str(stdin_ref.as_str()) {
                    Ok(hash) => hash,
                    Err(err) => {
                        let payload = HostExecReceipt {
                            exit_code: -1,
                            status: "error".into(),
                            stdout: None,
                            stderr: None,
                            started_at_ns,
                            ended_at_ns: now_wallclock_ns(),
                            error_code: Some("invalid_stdin_ref".into()),
                            error_message: Some(err.to_string()),
                        };
                        return Ok(build_receipt(
                            intent,
                            "host.exec",
                            ReceiptStatus::Error,
                            &payload,
                        )?);
                    }
                };
                match self.store.get_blob(hash) {
                    Ok(bytes) => Some(bytes),
                    Err(err) => {
                        let payload = HostExecReceipt {
                            exit_code: -1,
                            status: "error".into(),
                            stdout: None,
                            stderr: None,
                            started_at_ns,
                            ended_at_ns: now_wallclock_ns(),
                            error_code: Some("stdin_ref_not_found".into()),
                            error_message: Some(err.to_string()),
                        };
                        return Ok(build_receipt(
                            intent,
                            "host.exec",
                            ReceiptStatus::Error,
                            &payload,
                        )?);
                    }
                }
            }
            None => None,
        };

        if params.argv.is_empty() {
            let payload = HostExecReceipt {
                exit_code: -1,
                status: "error".into(),
                stdout: None,
                stderr: None,
                started_at_ns,
                ended_at_ns: now_wallclock_ns(),
                error_code: Some("argv_empty".into()),
                error_message: Some("argv must not be empty".into()),
            };
            return Ok(build_receipt(
                intent,
                "host.exec",
                ReceiptStatus::Error,
                &payload,
            )?);
        }

        let mut command = Command::new(&params.argv[0]);
        if params.argv.len() > 1 {
            command.args(&params.argv[1..]);
        }
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.stdin(Stdio::piped());

        if let Some(cwd) = params.cwd.as_deref().or(session.workdir.as_deref()) {
            command.current_dir(cwd);
        }

        for (key, value) in &session.env {
            command.env(key, value);
        }
        if let Some(env_patch) = &params.env_patch {
            for (key, value) in env_patch {
                command.env(key, value);
            }
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                let payload = HostExecReceipt {
                    exit_code: -1,
                    status: "error".into(),
                    stdout: None,
                    stderr: None,
                    started_at_ns,
                    ended_at_ns: now_wallclock_ns(),
                    error_code: Some("spawn_failed".into()),
                    error_message: Some(err.to_string()),
                };
                return Ok(build_receipt(
                    intent,
                    "host.exec",
                    ReceiptStatus::Error,
                    &payload,
                )?);
            }
        };

        let mut stdin = child.stdin.take();
        let Some(mut stdout) = child.stdout.take() else {
            let payload = HostExecReceipt {
                exit_code: -1,
                status: "error".into(),
                stdout: None,
                stderr: None,
                started_at_ns,
                ended_at_ns: now_wallclock_ns(),
                error_code: Some("stdout_capture_failed".into()),
                error_message: Some("missing child stdout pipe".into()),
            };
            return Ok(build_receipt(
                intent,
                "host.exec",
                ReceiptStatus::Error,
                &payload,
            )?);
        };
        let Some(mut stderr) = child.stderr.take() else {
            let payload = HostExecReceipt {
                exit_code: -1,
                status: "error".into(),
                stdout: None,
                stderr: None,
                started_at_ns,
                ended_at_ns: now_wallclock_ns(),
                error_code: Some("stderr_capture_failed".into()),
                error_message: Some("missing child stderr pipe".into()),
            };
            return Ok(build_receipt(
                intent,
                "host.exec",
                ReceiptStatus::Error,
                &payload,
            )?);
        };

        let stdout_task = tokio::spawn(async move {
            let mut out = Vec::new();
            let _ = stdout.read_to_end(&mut out).await;
            out
        });
        let stderr_task = tokio::spawn(async move {
            let mut out = Vec::new();
            let _ = stderr.read_to_end(&mut out).await;
            out
        });

        if let Some(bytes) = stdin_bytes {
            if let Some(mut child_stdin) = stdin.take() {
                let _ = child_stdin.write_all(&bytes).await;
            }
        }
        drop(stdin);

        let wait_outcome = match duration_from_ns(params.timeout_ns) {
            Some(timeout) => match tokio::time::timeout(timeout, child.wait()).await {
                Ok(status_result) => WaitOutcome::Completed(status_result),
                Err(_) => {
                    let _ = child.kill().await;
                    WaitOutcome::TimedOut(child.wait().await)
                }
            },
            None => WaitOutcome::Completed(child.wait().await),
        };

        let stdout_bytes = stdout_task.await.unwrap_or_default();
        let stderr_bytes = stderr_task.await.unwrap_or_default();
        let ended_at_ns = now_wallclock_ns();

        let (exit_code, status, receipt_status) = match wait_outcome {
            WaitOutcome::TimedOut(Ok(_)) => (-1, "timeout", ReceiptStatus::Timeout),
            WaitOutcome::TimedOut(Err(err)) => {
                let payload = HostExecReceipt {
                    exit_code: -1,
                    status: "error".into(),
                    stdout: None,
                    stderr: None,
                    started_at_ns,
                    ended_at_ns,
                    error_code: Some("wait_failed".into()),
                    error_message: Some(err.to_string()),
                };
                return Ok(build_receipt(
                    intent,
                    "host.exec",
                    ReceiptStatus::Error,
                    &payload,
                )?);
            }
            WaitOutcome::Completed(Err(err)) => {
                let payload = HostExecReceipt {
                    exit_code: -1,
                    status: "error".into(),
                    stdout: None,
                    stderr: None,
                    started_at_ns,
                    ended_at_ns,
                    error_code: Some("wait_failed".into()),
                    error_message: Some(err.to_string()),
                };
                return Ok(build_receipt(
                    intent,
                    "host.exec",
                    ReceiptStatus::Error,
                    &payload,
                )?);
            }
            WaitOutcome::Completed(Ok(exit_status)) => {
                if let Some(code) = exit_status.code() {
                    (code, "ok", ReceiptStatus::Ok)
                } else {
                    (-1, "signaled", ReceiptStatus::Ok)
                }
            }
        };

        let stdout = match materialize_output(self.store.as_ref(), mode, &stdout_bytes) {
            Ok(output) => output,
            Err(OutputMaterializeError::InlineRequiredTooLarge(len)) => {
                let payload = HostExecReceipt {
                    exit_code,
                    status: "error".into(),
                    stdout: None,
                    stderr: None,
                    started_at_ns,
                    ended_at_ns,
                    error_code: Some("inline_required_too_large".into()),
                    error_message: Some(format!(
                        "stdout {} bytes exceeds inline limit {}",
                        len, INLINE_OUTPUT_LIMIT_BYTES
                    )),
                };
                return Ok(build_receipt(
                    intent,
                    "host.exec",
                    ReceiptStatus::Error,
                    &payload,
                )?);
            }
            Err(OutputMaterializeError::Store(err)) => {
                let payload = HostExecReceipt {
                    exit_code,
                    status: "error".into(),
                    stdout: None,
                    stderr: None,
                    started_at_ns,
                    ended_at_ns,
                    error_code: Some("output_store_failed".into()),
                    error_message: Some(err),
                };
                return Ok(build_receipt(
                    intent,
                    "host.exec",
                    ReceiptStatus::Error,
                    &payload,
                )?);
            }
        };
        let stderr = match materialize_output(self.store.as_ref(), mode, &stderr_bytes) {
            Ok(output) => output,
            Err(OutputMaterializeError::InlineRequiredTooLarge(len)) => {
                let payload = HostExecReceipt {
                    exit_code,
                    status: "error".into(),
                    stdout,
                    stderr: None,
                    started_at_ns,
                    ended_at_ns,
                    error_code: Some("inline_required_too_large".into()),
                    error_message: Some(format!(
                        "stderr {} bytes exceeds inline limit {}",
                        len, INLINE_OUTPUT_LIMIT_BYTES
                    )),
                };
                return Ok(build_receipt(
                    intent,
                    "host.exec",
                    ReceiptStatus::Error,
                    &payload,
                )?);
            }
            Err(OutputMaterializeError::Store(err)) => {
                let payload = HostExecReceipt {
                    exit_code,
                    status: "error".into(),
                    stdout,
                    stderr: None,
                    started_at_ns,
                    ended_at_ns,
                    error_code: Some("output_store_failed".into()),
                    error_message: Some(err),
                };
                return Ok(build_receipt(
                    intent,
                    "host.exec",
                    ReceiptStatus::Error,
                    &payload,
                )?);
            }
        };

        {
            let mut state = self.state.lock().await;
            if let Some(session) = state.sessions.get_mut(&params.session_id) {
                session.last_exit_code = Some(exit_code);
            }
        }

        let payload = HostExecReceipt {
            exit_code,
            status: status.into(),
            stdout,
            stderr,
            started_at_ns,
            ended_at_ns,
            error_code: None,
            error_message: None,
        };

        build_receipt(intent, "host.exec", receipt_status, &payload)
    }
}

#[async_trait]
impl AsyncEffectAdapter for HostSessionSignalAdapter {
    fn kind(&self) -> &str {
        EffectKind::HOST_SESSION_SIGNAL
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostSessionSignalParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => {
                let payload = HostSessionSignalReceipt {
                    status: "error".into(),
                    exit_code: None,
                    ended_at_ns: None,
                    error_code: Some("invalid_params".into()),
                    error_message: Some(err.to_string()),
                };
                return Ok(build_receipt(
                    intent,
                    "host.session.signal",
                    ReceiptStatus::Error,
                    &payload,
                )?);
            }
        };

        let mut state = self.state.lock().await;
        let Some(session) = state.sessions.get_mut(&params.session_id) else {
            let payload = HostSessionSignalReceipt {
                status: "not_found".into(),
                exit_code: None,
                ended_at_ns: None,
                error_code: None,
                error_message: None,
            };
            return build_receipt(intent, "host.session.signal", ReceiptStatus::Ok, &payload);
        };

        if session.closed {
            let payload = HostSessionSignalReceipt {
                status: "already_exited".into(),
                exit_code: session.last_exit_code,
                ended_at_ns: session.ended_at_ns,
                error_code: None,
                error_message: None,
            };
            return build_receipt(intent, "host.session.signal", ReceiptStatus::Ok, &payload);
        }

        let ended_at_ns = now_wallclock_ns();
        session.closed = true;
        session.ended_at_ns = Some(ended_at_ns);

        let payload = HostSessionSignalReceipt {
            status: "signaled".into(),
            exit_code: session.last_exit_code,
            ended_at_ns: Some(ended_at_ns),
            error_code: None,
            error_message: None,
        };
        build_receipt(intent, "host.session.signal", ReceiptStatus::Ok, &payload)
    }
}

enum WaitOutcome {
    Completed(Result<std::process::ExitStatus, std::io::Error>),
    TimedOut(Result<std::process::ExitStatus, std::io::Error>),
}

enum OutputMaterializeError {
    InlineRequiredTooLarge(usize),
    Store(String),
}

fn materialize_output<S: Store>(
    store: &S,
    mode: &str,
    bytes: &[u8],
) -> Result<Option<HostOutput>, OutputMaterializeError> {
    if bytes.is_empty() {
        return Ok(None);
    }

    if mode == "require_inline" {
        if bytes.len() > INLINE_OUTPUT_LIMIT_BYTES {
            return Err(OutputMaterializeError::InlineRequiredTooLarge(bytes.len()));
        }
        return Ok(Some(to_inline_output(bytes)));
    }

    if bytes.len() <= INLINE_OUTPUT_LIMIT_BYTES {
        return Ok(Some(to_inline_output(bytes)));
    }

    let hash = store
        .put_blob(bytes)
        .map_err(|err| OutputMaterializeError::Store(err.to_string()))?;
    let blob_ref = HashRef::new(hash.to_hex())
        .map_err(|err| OutputMaterializeError::Store(err.to_string()))?;
    let preview = bytes[..bytes.len().min(OUTPUT_PREVIEW_BYTES)].to_vec();
    Ok(Some(HostOutput::Blob {
        blob: HostBlobOutput {
            blob_ref,
            size_bytes: bytes.len() as u64,
            preview_bytes: Some(preview),
        },
    }))
}

fn to_inline_output(bytes: &[u8]) -> HostOutput {
    match std::str::from_utf8(bytes) {
        Ok(text) => HostOutput::InlineText {
            inline_text: HostInlineText {
                text: text.to_string(),
            },
        },
        Err(_) => HostOutput::InlineBytes {
            inline_bytes: HostInlineBytes {
                bytes: bytes.to_vec(),
            },
        },
    }
}

fn duration_from_ns(ns: Option<u64>) -> Option<Duration> {
    ns.map(Duration::from_nanos)
}

fn now_wallclock_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

fn build_receipt<T: serde::Serialize>(
    intent: &EffectIntent,
    adapter_id: &str,
    status: ReceiptStatus,
    payload: &T,
) -> anyhow::Result<EffectReceipt> {
    Ok(EffectReceipt {
        intent_hash: intent.intent_hash,
        adapter_id: adapter_id.into(),
        status,
        payload_cbor: serde_cbor::to_vec(payload)
            .with_context(|| format!("encode {} payload", intent.kind.as_str()))?,
        cost_cents: Some(0),
        signature: vec![0; 64],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::{builtins::builtin_schemas, schema_index::SchemaIndex};
    use aos_effects::builtins::{HostLocalTarget, HostTarget};
    use aos_store::MemStore;
    use std::collections::HashMap;

    fn intent_for<T: serde::Serialize>(kind: &str, params: &T, seed: u8) -> EffectIntent {
        EffectIntent::from_raw_params(
            EffectKind::new(kind),
            "cap_host",
            serde_cbor::to_vec(params).expect("encode params"),
            [seed; 32],
        )
        .expect("intent")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn process_session_open_exec_signal_roundtrip() {
        let store = Arc::new(MemStore::new());
        let (open_adapter, exec_adapter, signal_adapter) = make_host_adapters(store);

        let open_params = HostSessionOpenParams {
            target: HostTarget {
                local: Some(HostLocalTarget {
                    mounts: None,
                    workdir: None,
                    env: None,
                    network_mode: "none".into(),
                }),
            },
            session_ttl_ns: None,
            labels: None,
        };

        let open_receipt = open_adapter
            .execute(&intent_for(EffectKind::HOST_SESSION_OPEN, &open_params, 1))
            .await
            .expect("open receipt");
        assert_eq!(open_receipt.status, ReceiptStatus::Ok);
        assert_schema_normalizes("sys/HostSessionOpenReceipt@1", &open_receipt.payload_cbor);
        let open_payload: HostSessionOpenReceipt =
            serde_cbor::from_slice(&open_receipt.payload_cbor).expect("decode open payload");
        assert_eq!(open_payload.status, "ready");
        assert!(!open_payload.session_id.is_empty());

        let exec_params = HostExecParams {
            session_id: open_payload.session_id.clone(),
            argv: vec!["/bin/sh".into(), "-lc".into(), "printf 'hello'".into()],
            cwd: None,
            timeout_ns: None,
            env_patch: None,
            stdin_ref: None,
            output_mode: Some("require_inline".into()),
        };
        let exec_receipt = exec_adapter
            .execute(&intent_for(EffectKind::HOST_EXEC, &exec_params, 2))
            .await
            .expect("exec receipt");
        assert_eq!(exec_receipt.status, ReceiptStatus::Ok);
        assert_schema_normalizes("sys/HostExecReceipt@1", &exec_receipt.payload_cbor);
        let exec_payload: HostExecReceipt =
            serde_cbor::from_slice(&exec_receipt.payload_cbor).expect("decode exec payload");
        assert_eq!(exec_payload.status, "ok");
        assert_eq!(exec_payload.exit_code, 0);
        match exec_payload.stdout {
            Some(HostOutput::InlineText { inline_text }) => {
                assert_eq!(inline_text.text, "hello")
            }
            other => panic!("expected inline_text stdout, got {other:?}"),
        }

        let signal_params = HostSessionSignalParams {
            session_id: open_payload.session_id.clone(),
            signal: "term".into(),
            grace_timeout_ns: None,
        };
        let signal_receipt = signal_adapter
            .execute(&intent_for(
                EffectKind::HOST_SESSION_SIGNAL,
                &signal_params,
                3,
            ))
            .await
            .expect("signal receipt");
        assert_eq!(signal_receipt.status, ReceiptStatus::Ok);
        assert_schema_normalizes(
            "sys/HostSessionSignalReceipt@1",
            &signal_receipt.payload_cbor,
        );
        let signal_payload: HostSessionSignalReceipt =
            serde_cbor::from_slice(&signal_receipt.payload_cbor).expect("decode signal payload");
        assert_eq!(signal_payload.status, "signaled");

        let exec_after_close = exec_adapter
            .execute(&intent_for(EffectKind::HOST_EXEC, &exec_params, 4))
            .await
            .expect("exec after close receipt");
        assert_eq!(exec_after_close.status, ReceiptStatus::Error);
        let exec_after_close_payload: HostExecReceipt =
            serde_cbor::from_slice(&exec_after_close.payload_cbor)
                .expect("decode exec after close payload");
        assert_eq!(exec_after_close_payload.status, "error");
        assert_eq!(
            exec_after_close_payload.error_code.as_deref(),
            Some("session_closed")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn process_exec_require_inline_rejects_large_output() {
        let store = Arc::new(MemStore::new());
        let (open_adapter, exec_adapter, _) = make_host_adapters(store);

        let open_params = HostSessionOpenParams {
            target: HostTarget {
                local: Some(HostLocalTarget {
                    mounts: None,
                    workdir: None,
                    env: None,
                    network_mode: "none".into(),
                }),
            },
            session_ttl_ns: None,
            labels: None,
        };

        let open_receipt = open_adapter
            .execute(&intent_for(EffectKind::HOST_SESSION_OPEN, &open_params, 10))
            .await
            .expect("open receipt");
        let open_payload: HostSessionOpenReceipt =
            serde_cbor::from_slice(&open_receipt.payload_cbor).expect("decode open payload");

        let exec_params = HostExecParams {
            session_id: open_payload.session_id,
            argv: vec![
                "/bin/sh".into(),
                "-lc".into(),
                "head -c 20000 /dev/zero".into(),
            ],
            cwd: None,
            timeout_ns: None,
            env_patch: None,
            stdin_ref: None,
            output_mode: Some("require_inline".into()),
        };

        let exec_receipt = exec_adapter
            .execute(&intent_for(EffectKind::HOST_EXEC, &exec_params, 11))
            .await
            .expect("exec receipt");
        assert_eq!(exec_receipt.status, ReceiptStatus::Error);
        assert_schema_normalizes("sys/HostExecReceipt@1", &exec_receipt.payload_cbor);
        let exec_payload: HostExecReceipt =
            serde_cbor::from_slice(&exec_receipt.payload_cbor).expect("decode exec payload");
        assert_eq!(exec_payload.status, "error");
        assert_eq!(
            exec_payload.error_code.as_deref(),
            Some("inline_required_too_large")
        );
    }

    fn assert_schema_normalizes(schema_name: &str, payload: &[u8]) {
        let mut schemas = HashMap::new();
        for entry in builtin_schemas() {
            schemas.insert(entry.schema.name.clone(), entry.schema.ty.clone());
        }
        let index = SchemaIndex::new(schemas);
        aos_air_types::value_normalize::normalize_cbor_by_name(&index, schema_name, payload)
            .expect("payload must normalize against builtin schema");
    }
}
