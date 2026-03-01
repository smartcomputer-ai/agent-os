use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use aos_cbor::Hash;
use aos_effects::builtins::{
    HostExecParams, HostExecReceipt, HostFileContentInput, HostFsApplyPatchParams,
    HostFsApplyPatchReceipt, HostFsEditFileParams, HostFsEditFileReceipt, HostFsExistsParams,
    HostFsExistsReceipt, HostFsGlobParams, HostFsGlobReceipt, HostFsGrepParams, HostFsGrepReceipt,
    HostFsListDirParams, HostFsListDirReceipt, HostFsReadFileParams, HostFsReadFileReceipt,
    HostFsStatParams, HostFsStatReceipt, HostFsWriteFileParams, HostFsWriteFileReceipt, HostOutput,
    HostPatchOpsSummary, HostSessionOpenParams, HostSessionOpenReceipt, HostSessionSignalParams,
    HostSessionSignalReceipt,
};
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use aos_store::Store;
use async_trait::async_trait;
use globset::Glob;
use regex::RegexBuilder;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::Mutex;
use walkdir::WalkDir;

use super::traits::AsyncEffectAdapter;

mod output;
mod patch;
mod paths;
mod state;

use output::{
    OutputConfig, OutputMaterializeError, materialize_binary_output, materialize_output,
    materialize_text_output, output_mode_valid,
};
use patch::{
    EditMatchError, ParsedPatch, PatchOpCounts, PatchOperation, apply_edit, apply_update_hunks,
    parse_patch_v4a,
};
use paths::{PathResolveError, display_relative, resolve_session_base, resolve_session_path};
use state::{HostState, SessionRecord};

const DEFAULT_GREP_MAX_RESULTS: usize = 100;
const DEFAULT_GLOB_MAX_RESULTS: usize = 100;

pub struct HostAdapterSet<S: Store> {
    pub session_open: HostSessionOpenAdapter,
    pub exec: HostExecAdapter<S>,
    pub session_signal: HostSessionSignalAdapter,
    pub fs_read_file: HostFsReadFileAdapter<S>,
    pub fs_write_file: HostFsWriteFileAdapter<S>,
    pub fs_edit_file: HostFsEditFileAdapter,
    pub fs_apply_patch: HostFsApplyPatchAdapter<S>,
    pub fs_grep: HostFsGrepAdapter<S>,
    pub fs_glob: HostFsGlobAdapter<S>,
    pub fs_stat: HostFsStatAdapter,
    pub fs_exists: HostFsExistsAdapter,
    pub fs_list_dir: HostFsListDirAdapter<S>,
}

pub struct HostSessionOpenAdapter {
    state: Arc<Mutex<HostState>>,
}

pub struct HostExecAdapter<S: Store> {
    state: Arc<Mutex<HostState>>,
    store: Arc<S>,
    output_cfg: OutputConfig,
}

pub struct HostSessionSignalAdapter {
    state: Arc<Mutex<HostState>>,
}

pub struct HostFsReadFileAdapter<S: Store> {
    state: Arc<Mutex<HostState>>,
    store: Arc<S>,
    output_cfg: OutputConfig,
}

pub struct HostFsWriteFileAdapter<S: Store> {
    state: Arc<Mutex<HostState>>,
    store: Arc<S>,
}

pub struct HostFsEditFileAdapter {
    state: Arc<Mutex<HostState>>,
}

pub struct HostFsApplyPatchAdapter<S: Store> {
    state: Arc<Mutex<HostState>>,
    store: Arc<S>,
}

pub struct HostFsGrepAdapter<S: Store> {
    state: Arc<Mutex<HostState>>,
    store: Arc<S>,
    output_cfg: OutputConfig,
}

pub struct HostFsGlobAdapter<S: Store> {
    state: Arc<Mutex<HostState>>,
    store: Arc<S>,
    output_cfg: OutputConfig,
}

pub struct HostFsStatAdapter {
    state: Arc<Mutex<HostState>>,
}

pub struct HostFsExistsAdapter {
    state: Arc<Mutex<HostState>>,
}

pub struct HostFsListDirAdapter<S: Store> {
    state: Arc<Mutex<HostState>>,
    store: Arc<S>,
    output_cfg: OutputConfig,
}

pub fn make_host_adapter_set<S: Store + Send + Sync + 'static>(store: Arc<S>) -> HostAdapterSet<S> {
    let state = Arc::new(Mutex::new(HostState::default()));
    let output_cfg = OutputConfig::default();
    HostAdapterSet {
        session_open: HostSessionOpenAdapter {
            state: state.clone(),
        },
        exec: HostExecAdapter {
            state: state.clone(),
            store: store.clone(),
            output_cfg,
        },
        session_signal: HostSessionSignalAdapter {
            state: state.clone(),
        },
        fs_read_file: HostFsReadFileAdapter {
            state: state.clone(),
            store: store.clone(),
            output_cfg,
        },
        fs_write_file: HostFsWriteFileAdapter {
            state: state.clone(),
            store: store.clone(),
        },
        fs_edit_file: HostFsEditFileAdapter {
            state: state.clone(),
        },
        fs_apply_patch: HostFsApplyPatchAdapter {
            state: state.clone(),
            store: store.clone(),
        },
        fs_grep: HostFsGrepAdapter {
            state: state.clone(),
            store: store.clone(),
            output_cfg,
        },
        fs_glob: HostFsGlobAdapter {
            state: state.clone(),
            store: store.clone(),
            output_cfg,
        },
        fs_stat: HostFsStatAdapter {
            state: state.clone(),
        },
        fs_exists: HostFsExistsAdapter {
            state: state.clone(),
        },
        fs_list_dir: HostFsListDirAdapter {
            state,
            store,
            output_cfg,
        },
    }
}

pub fn make_host_adapters<S: Store + Send + Sync + 'static>(
    store: Arc<S>,
) -> (
    HostSessionOpenAdapter,
    HostExecAdapter<S>,
    HostSessionSignalAdapter,
) {
    let set = make_host_adapter_set(store);
    (set.session_open, set.exec, set.session_signal)
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
                    EffectKind::HOST_SESSION_OPEN,
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
                EffectKind::HOST_SESSION_OPEN,
                ReceiptStatus::Error,
                &payload,
            )?);
        };

        let requested_workdir = target_local
            .workdir
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));
        let workdir = match std::fs::canonicalize(&requested_workdir) {
            Ok(path) => path,
            Err(err) => {
                let payload = HostSessionOpenReceipt {
                    session_id: String::new(),
                    status: "error".into(),
                    started_at_ns,
                    expires_at_ns: None,
                    error_code: Some("invalid_workdir".into()),
                    error_message: Some(format!(
                        "cannot open workdir '{}': {err}",
                        requested_workdir.to_string_lossy()
                    )),
                };
                return Ok(build_receipt(
                    intent,
                    EffectKind::HOST_SESSION_OPEN,
                    ReceiptStatus::Error,
                    &payload,
                )?);
            }
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
                workdir,
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

        build_receipt(
            intent,
            EffectKind::HOST_SESSION_OPEN,
            ReceiptStatus::Ok,
            &payload,
        )
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
                return exec_error_receipt(
                    intent,
                    started_at_ns,
                    -1,
                    "invalid_params",
                    err.to_string(),
                    None,
                );
            }
        };

        let session = match active_session(&self.state, &params.session_id).await {
            Ok(session) => session,
            Err(err) => {
                return exec_error_receipt(intent, started_at_ns, -1, err.code, err.message, None);
            }
        };

        let mode = params.output_mode.as_deref().unwrap_or("auto");
        if !output_mode_valid(mode) {
            return exec_error_receipt(
                intent,
                started_at_ns,
                -1,
                "invalid_output_mode",
                format!("unsupported output_mode '{mode}'"),
                None,
            );
        }

        let stdin_bytes = match params.stdin_ref.as_ref() {
            Some(stdin_ref) => {
                let hash = match Hash::from_hex_str(stdin_ref.as_str()) {
                    Ok(hash) => hash,
                    Err(err) => {
                        return exec_error_receipt(
                            intent,
                            started_at_ns,
                            -1,
                            "invalid_stdin_ref",
                            err.to_string(),
                            None,
                        );
                    }
                };
                match self.store.get_blob(hash) {
                    Ok(bytes) => Some(bytes),
                    Err(err) => {
                        return exec_error_receipt(
                            intent,
                            started_at_ns,
                            -1,
                            "stdin_ref_not_found",
                            err.to_string(),
                            None,
                        );
                    }
                }
            }
            None => None,
        };

        if params.argv.is_empty() {
            return exec_error_receipt(
                intent,
                started_at_ns,
                -1,
                "argv_empty",
                "argv must not be empty".into(),
                None,
            );
        }

        let mut command = Command::new(&params.argv[0]);
        if params.argv.len() > 1 {
            command.args(&params.argv[1..]);
        }
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.stdin(Stdio::piped());

        if let Some(cwd) = params.cwd.as_deref() {
            command.current_dir(&cwd);
        } else {
            command.current_dir(&session.workdir);
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
                return exec_error_receipt(
                    intent,
                    started_at_ns,
                    -1,
                    "spawn_failed",
                    err.to_string(),
                    None,
                );
            }
        };

        let mut stdin = child.stdin.take();
        let Some(mut stdout) = child.stdout.take() else {
            return exec_error_receipt(
                intent,
                started_at_ns,
                -1,
                "stdout_capture_failed",
                "missing child stdout pipe".into(),
                None,
            );
        };
        let Some(mut stderr) = child.stderr.take() else {
            return exec_error_receipt(
                intent,
                started_at_ns,
                -1,
                "stderr_capture_failed",
                "missing child stderr pipe".into(),
                None,
            );
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
                return exec_error_receipt(
                    intent,
                    started_at_ns,
                    -1,
                    "wait_failed",
                    err.to_string(),
                    None,
                );
            }
            WaitOutcome::Completed(Err(err)) => {
                return exec_error_receipt(
                    intent,
                    started_at_ns,
                    -1,
                    "wait_failed",
                    err.to_string(),
                    None,
                );
            }
            WaitOutcome::Completed(Ok(exit_status)) => {
                if let Some(code) = exit_status.code() {
                    (code, "ok", ReceiptStatus::Ok)
                } else {
                    (-1, "signaled", ReceiptStatus::Ok)
                }
            }
        };

        let stdout =
            match materialize_output(self.store.as_ref(), mode, &stdout_bytes, self.output_cfg) {
                Ok(output) => output,
                Err(OutputMaterializeError::InlineRequiredTooLarge(len)) => {
                    return exec_error_receipt(
                        intent,
                        started_at_ns,
                        exit_code,
                        "inline_required_too_large",
                        format!(
                            "stdout {len} bytes exceeds inline limit {}",
                            self.output_cfg.inline_limit_bytes
                        ),
                        None,
                    );
                }
                Err(OutputMaterializeError::Store(err)) => {
                    return exec_error_receipt(
                        intent,
                        started_at_ns,
                        exit_code,
                        "output_store_failed",
                        err,
                        None,
                    );
                }
            };
        let stderr =
            match materialize_output(self.store.as_ref(), mode, &stderr_bytes, self.output_cfg) {
                Ok(output) => output,
                Err(OutputMaterializeError::InlineRequiredTooLarge(len)) => {
                    return exec_error_receipt(
                        intent,
                        started_at_ns,
                        exit_code,
                        "inline_required_too_large",
                        format!(
                            "stderr {len} bytes exceeds inline limit {}",
                            self.output_cfg.inline_limit_bytes
                        ),
                        stdout,
                    );
                }
                Err(OutputMaterializeError::Store(err)) => {
                    return exec_error_receipt(
                        intent,
                        started_at_ns,
                        exit_code,
                        "output_store_failed",
                        err,
                        stdout,
                    );
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

        build_receipt(intent, EffectKind::HOST_EXEC, receipt_status, &payload)
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
                    EffectKind::HOST_SESSION_SIGNAL,
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
            return build_receipt(
                intent,
                EffectKind::HOST_SESSION_SIGNAL,
                ReceiptStatus::Ok,
                &payload,
            );
        };

        if session.closed {
            let payload = HostSessionSignalReceipt {
                status: "already_exited".into(),
                exit_code: session.last_exit_code,
                ended_at_ns: session.ended_at_ns,
                error_code: None,
                error_message: None,
            };
            return build_receipt(
                intent,
                EffectKind::HOST_SESSION_SIGNAL,
                ReceiptStatus::Ok,
                &payload,
            );
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
        build_receipt(
            intent,
            EffectKind::HOST_SESSION_SIGNAL,
            ReceiptStatus::Ok,
            &payload,
        )
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for HostFsReadFileAdapter<S> {
    fn kind(&self) -> &str {
        EffectKind::HOST_FS_READ_FILE
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostFsReadFileParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fs_read_error(intent, "invalid_params", err.to_string()),
        };

        let mode = params.output_mode.as_deref().unwrap_or("auto");
        if !output_mode_valid(mode) {
            return fs_read_error(
                intent,
                "invalid_output_mode",
                format!("unsupported output_mode '{mode}'"),
            );
        }

        let session = match active_session(&self.state, &params.session_id).await {
            Ok(session) => session,
            Err(err) => {
                return fs_read_error(intent, err.code, err.message);
            }
        };

        let path = match resolve_session_path(&session, &params.path) {
            Ok(path) => path,
            Err(err) => {
                let status = if err.code == "forbidden" {
                    "forbidden"
                } else {
                    "error"
                };
                let payload = HostFsReadFileReceipt {
                    status: status.into(),
                    content: None,
                    truncated: None,
                    size_bytes: None,
                    error_code: Some(err.code.into()),
                };
                let receipt_status = if status == "error" {
                    ReceiptStatus::Error
                } else {
                    ReceiptStatus::Ok
                };
                return build_receipt(
                    intent,
                    EffectKind::HOST_FS_READ_FILE,
                    receipt_status,
                    &payload,
                );
            }
        };

        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let payload = HostFsReadFileReceipt {
                    status: "not_found".into(),
                    content: None,
                    truncated: None,
                    size_bytes: None,
                    error_code: None,
                };
                return build_receipt(
                    intent,
                    EffectKind::HOST_FS_READ_FILE,
                    ReceiptStatus::Ok,
                    &payload,
                );
            }
            Err(err) => return fs_read_error(intent, "read_failed", err.to_string()),
        };

        if metadata.is_dir() {
            let payload = HostFsReadFileReceipt {
                status: "is_directory".into(),
                content: None,
                truncated: None,
                size_bytes: None,
                error_code: None,
            };
            return build_receipt(
                intent,
                EffectKind::HOST_FS_READ_FILE,
                ReceiptStatus::Ok,
                &payload,
            );
        }

        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(err) => return fs_read_error(intent, "read_failed", err.to_string()),
        };

        let size_bytes = bytes.len() as u64;
        let offset = params.offset_bytes.unwrap_or(0).min(size_bytes) as usize;
        let max_len = params.max_bytes.unwrap_or(u64::MAX).min(usize::MAX as u64) as usize;
        let end = offset.saturating_add(max_len).min(bytes.len());
        let slice = &bytes[offset..end];
        let truncated = end < bytes.len();
        let encoding = params.encoding.as_deref().unwrap_or("utf8");

        let content = if encoding == "bytes" {
            match materialize_binary_output(self.store.as_ref(), mode, slice, self.output_cfg) {
                Ok(content) => content,
                Err(OutputMaterializeError::InlineRequiredTooLarge(len)) => {
                    let _ = len;
                    let payload = HostFsReadFileReceipt {
                        status: "error".into(),
                        content: None,
                        truncated: None,
                        size_bytes: Some(size_bytes),
                        error_code: Some("inline_required_too_large".into()),
                    };
                    return build_receipt(
                        intent,
                        EffectKind::HOST_FS_READ_FILE,
                        ReceiptStatus::Error,
                        &payload,
                    );
                }
                Err(OutputMaterializeError::Store(err)) => {
                    return fs_read_error(intent, "output_store_failed", err);
                }
            }
        } else if encoding == "utf8" {
            let text = match std::str::from_utf8(slice) {
                Ok(text) => text,
                Err(_) => {
                    let payload = HostFsReadFileReceipt {
                        status: "error".into(),
                        content: None,
                        truncated: None,
                        size_bytes: Some(size_bytes),
                        error_code: Some("invalid_utf8".into()),
                    };
                    return build_receipt(
                        intent,
                        EffectKind::HOST_FS_READ_FILE,
                        ReceiptStatus::Error,
                        &payload,
                    );
                }
            };
            match materialize_text_output(self.store.as_ref(), mode, text, self.output_cfg) {
                Ok(Some(aos_effects::builtins::HostTextOutput::InlineText { inline_text })) => {
                    Some(HostOutput::InlineText { inline_text })
                }
                Ok(Some(aos_effects::builtins::HostTextOutput::Blob { blob })) => {
                    Some(HostOutput::Blob { blob })
                }
                Ok(None) => None,
                Err(OutputMaterializeError::InlineRequiredTooLarge(len)) => {
                    let _ = len;
                    let payload = HostFsReadFileReceipt {
                        status: "error".into(),
                        content: None,
                        truncated: None,
                        size_bytes: Some(size_bytes),
                        error_code: Some("inline_required_too_large".into()),
                    };
                    return build_receipt(
                        intent,
                        EffectKind::HOST_FS_READ_FILE,
                        ReceiptStatus::Error,
                        &payload,
                    );
                }
                Err(OutputMaterializeError::Store(err)) => {
                    return fs_read_error(intent, "output_store_failed", err);
                }
            }
        } else {
            return fs_read_error(
                intent,
                "invalid_encoding",
                format!("unsupported encoding '{encoding}'"),
            );
        };

        let payload = HostFsReadFileReceipt {
            status: "ok".into(),
            content,
            truncated: Some(truncated),
            size_bytes: Some(size_bytes),
            error_code: None,
        };
        build_receipt(
            intent,
            EffectKind::HOST_FS_READ_FILE,
            ReceiptStatus::Ok,
            &payload,
        )
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for HostFsWriteFileAdapter<S> {
    fn kind(&self) -> &str {
        EffectKind::HOST_FS_WRITE_FILE
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostFsWriteFileParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fs_write_error(intent, "invalid_params", err.to_string()),
        };

        let session = match active_session(&self.state, &params.session_id).await {
            Ok(session) => session,
            Err(err) => return fs_write_error(intent, err.code, err.message),
        };

        let path = match resolve_session_path(&session, &params.path) {
            Ok(path) => path,
            Err(err) => {
                let status = if err.code == "forbidden" {
                    "forbidden"
                } else {
                    "error"
                };
                let payload = HostFsWriteFileReceipt {
                    status: status.into(),
                    written_bytes: None,
                    created: None,
                    new_mtime_ns: None,
                    error_code: Some(err.code.into()),
                };
                let receipt_status = if status == "error" {
                    ReceiptStatus::Error
                } else {
                    ReceiptStatus::Ok
                };
                return build_receipt(
                    intent,
                    EffectKind::HOST_FS_WRITE_FILE,
                    receipt_status,
                    &payload,
                );
            }
        };

        let bytes = match self.resolve_file_content(&params.content) {
            Ok(bytes) => bytes,
            Err(err) => return fs_write_error(intent, "invalid_content", err),
        };

        let mode = params.mode.as_deref().unwrap_or("overwrite");
        if mode != "overwrite" && mode != "create_new" {
            return fs_write_error(intent, "invalid_mode", format!("unsupported mode '{mode}'"));
        }

        let existed = path.exists();
        if mode == "create_new" && existed {
            let payload = HostFsWriteFileReceipt {
                status: "conflict".into(),
                written_bytes: None,
                created: None,
                new_mtime_ns: None,
                error_code: Some("file_exists".into()),
            };
            return build_receipt(
                intent,
                EffectKind::HOST_FS_WRITE_FILE,
                ReceiptStatus::Ok,
                &payload,
            );
        }

        let create_parents = params.create_parents.unwrap_or(false);
        if let Some(parent) = path.parent() {
            if !parent.exists() && !create_parents {
                return fs_write_error(
                    intent,
                    "parent_not_found",
                    format!("parent '{}' does not exist", parent.to_string_lossy()),
                );
            }
            if create_parents {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|err| anyhow::anyhow!(err))?;
            }
        }

        write_file_atomic(&path, &bytes)
            .await
            .with_context(|| format!("write file '{}')", path.to_string_lossy()))?;
        let metadata = tokio::fs::metadata(&path).await?;
        let new_mtime_ns = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos() as u64);

        let payload = HostFsWriteFileReceipt {
            status: "ok".into(),
            written_bytes: Some(bytes.len() as u64),
            created: Some(!existed),
            new_mtime_ns,
            error_code: None,
        };
        build_receipt(
            intent,
            EffectKind::HOST_FS_WRITE_FILE,
            ReceiptStatus::Ok,
            &payload,
        )
    }
}

#[async_trait]
impl AsyncEffectAdapter for HostFsEditFileAdapter {
    fn kind(&self) -> &str {
        EffectKind::HOST_FS_EDIT_FILE
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostFsEditFileParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fs_edit_error(intent, "invalid_params", err.to_string()),
        };

        if params.old_string.is_empty() {
            return fs_edit_error(
                intent,
                "invalid_input_empty_old_string",
                "old_string must not be empty".into(),
            );
        }

        let session = match active_session(&self.state, &params.session_id).await {
            Ok(session) => session,
            Err(err) => return fs_edit_error(intent, err.code, err.message),
        };

        let path = match resolve_session_path(&session, &params.path) {
            Ok(path) => path,
            Err(err) => {
                let status = if err.code == "forbidden" {
                    "forbidden"
                } else {
                    "error"
                };
                let payload = HostFsEditFileReceipt {
                    status: status.into(),
                    replacements: None,
                    applied: None,
                    summary_text: None,
                    error_code: Some(err.code.into()),
                };
                let receipt_status = if status == "error" {
                    ReceiptStatus::Error
                } else {
                    ReceiptStatus::Ok
                };
                return build_receipt(
                    intent,
                    EffectKind::HOST_FS_EDIT_FILE,
                    receipt_status,
                    &payload,
                );
            }
        };

        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let payload = HostFsEditFileReceipt {
                    status: "not_found".into(),
                    replacements: None,
                    applied: Some(false),
                    summary_text: None,
                    error_code: Some("path_not_found".into()),
                };
                return build_receipt(
                    intent,
                    EffectKind::HOST_FS_EDIT_FILE,
                    ReceiptStatus::Ok,
                    &payload,
                );
            }
            Err(err) => return fs_edit_error(intent, "read_failed", err.to_string()),
        };

        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                let payload = HostFsEditFileReceipt {
                    status: "error".into(),
                    replacements: None,
                    applied: Some(false),
                    summary_text: None,
                    error_code: Some("invalid_utf8".into()),
                };
                return build_receipt(
                    intent,
                    EffectKind::HOST_FS_EDIT_FILE,
                    ReceiptStatus::Error,
                    &payload,
                );
            }
        };

        let replace_all = params.replace_all.unwrap_or(false);
        let edit = apply_edit(&text, &params.old_string, &params.new_string, replace_all);
        match edit {
            Err(EditMatchError::NotFound) => {
                let payload = HostFsEditFileReceipt {
                    status: "not_found".into(),
                    replacements: Some(0),
                    applied: Some(false),
                    summary_text: None,
                    error_code: Some("edit_target_not_found".into()),
                };
                build_receipt(
                    intent,
                    EffectKind::HOST_FS_EDIT_FILE,
                    ReceiptStatus::Ok,
                    &payload,
                )
            }
            Err(EditMatchError::Ambiguous(count)) => {
                let payload = HostFsEditFileReceipt {
                    status: "ambiguous".into(),
                    replacements: Some(count as u64),
                    applied: Some(false),
                    summary_text: None,
                    error_code: Some("ambiguous_matches".into()),
                };
                build_receipt(
                    intent,
                    EffectKind::HOST_FS_EDIT_FILE,
                    ReceiptStatus::Ok,
                    &payload,
                )
            }
            Ok(result) => {
                write_file_atomic(&path, result.updated.as_bytes()).await?;
                let summary = format!(
                    "Updated {} ({} replacement{})",
                    display_relative(&session.workdir, &path),
                    result.replacements,
                    if result.replacements == 1 { "" } else { "s" }
                );
                let payload = HostFsEditFileReceipt {
                    status: "ok".into(),
                    replacements: Some(result.replacements as u64),
                    applied: Some(true),
                    summary_text: Some(summary),
                    error_code: None,
                };
                build_receipt(
                    intent,
                    EffectKind::HOST_FS_EDIT_FILE,
                    ReceiptStatus::Ok,
                    &payload,
                )
            }
        }
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for HostFsApplyPatchAdapter<S> {
    fn kind(&self) -> &str {
        EffectKind::HOST_FS_APPLY_PATCH
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostFsApplyPatchParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fs_apply_patch_error(intent, "invalid_params", err.to_string()),
        };

        let patch_format = params.patch_format.as_deref().unwrap_or("v4a");
        if patch_format != "v4a" {
            let payload = HostFsApplyPatchReceipt {
                status: "error".into(),
                files_changed: None,
                changed_paths: None,
                ops: None,
                summary_text: None,
                errors: None,
                error_code: Some("unsupported_patch_format".into()),
            };
            return build_receipt(
                intent,
                EffectKind::HOST_FS_APPLY_PATCH,
                ReceiptStatus::Error,
                &payload,
            );
        }

        let session = match active_session(&self.state, &params.session_id).await {
            Ok(session) => session,
            Err(err) => return fs_apply_patch_error(intent, err.code, err.message),
        };

        let patch_text = match resolve_patch_text(self.store.as_ref(), &params) {
            Ok(text) => text,
            Err(err) => return fs_apply_patch_error(intent, "invalid_patch", err),
        };
        if patch_text.trim().is_empty() {
            return fs_apply_patch_error(
                intent,
                "invalid_patch_empty",
                "patch must not be empty".into(),
            );
        }

        let parsed = match parse_patch_v4a(&patch_text) {
            Ok(parsed) => parsed,
            Err(err) => {
                let payload = HostFsApplyPatchReceipt {
                    status: "parse_error".into(),
                    files_changed: None,
                    changed_paths: None,
                    ops: None,
                    summary_text: None,
                    errors: Some(vec![err]),
                    error_code: Some("patch_parse_error".into()),
                };
                return build_receipt(
                    intent,
                    EffectKind::HOST_FS_APPLY_PATCH,
                    ReceiptStatus::Ok,
                    &payload,
                );
            }
        };

        let dry_run = params.dry_run.unwrap_or(false);
        let applied = match apply_patch_to_session(&session, &parsed, dry_run).await {
            Ok(applied) => applied,
            Err(err) => {
                let payload = HostFsApplyPatchReceipt {
                    status: err.status,
                    files_changed: None,
                    changed_paths: None,
                    ops: None,
                    summary_text: None,
                    errors: Some(vec![err.message]),
                    error_code: Some(err.error_code),
                };
                return build_receipt(
                    intent,
                    EffectKind::HOST_FS_APPLY_PATCH,
                    ReceiptStatus::Ok,
                    &payload,
                );
            }
        };

        let ops = HostPatchOpsSummary {
            add: applied.counts.add,
            update: applied.counts.update,
            delete: applied.counts.delete,
            r#move: applied.counts.move_count,
        };
        let summary = if dry_run {
            format!(
                "Patch dry-run: {} file(s) would change ({})",
                applied.files_changed, applied.counts
            )
        } else {
            format!(
                "Applied patch: {} file(s) changed ({})",
                applied.files_changed, applied.counts
            )
        };

        let payload = HostFsApplyPatchReceipt {
            status: "ok".into(),
            files_changed: Some(applied.files_changed),
            changed_paths: Some(applied.changed_paths),
            ops: Some(ops),
            summary_text: Some(summary),
            errors: None,
            error_code: None,
        };
        build_receipt(
            intent,
            EffectKind::HOST_FS_APPLY_PATCH,
            ReceiptStatus::Ok,
            &payload,
        )
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for HostFsGrepAdapter<S> {
    fn kind(&self) -> &str {
        EffectKind::HOST_FS_GREP
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostFsGrepParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fs_grep_error(intent, "invalid_params", err.to_string()),
        };
        let mode = params.output_mode.as_deref().unwrap_or("auto");
        if !output_mode_valid(mode) {
            return fs_grep_error(
                intent,
                "invalid_output_mode",
                format!("unsupported output_mode '{mode}'"),
            );
        }

        let session = match active_session(&self.state, &params.session_id).await {
            Ok(session) => session,
            Err(err) => return fs_grep_error(intent, err.code, err.message),
        };
        let base = match resolve_session_base(&session, params.path.as_deref()) {
            Ok(path) => path,
            Err(err) => {
                let payload = HostFsGrepReceipt {
                    status: if err.code == "forbidden" {
                        "forbidden"
                    } else {
                        "error"
                    }
                    .into(),
                    matches: None,
                    match_count: None,
                    truncated: None,
                    error_code: Some(err.code.into()),
                    summary_text: None,
                };
                let receipt_status = if err.code == "forbidden" {
                    ReceiptStatus::Ok
                } else {
                    ReceiptStatus::Error
                };
                return build_receipt(intent, EffectKind::HOST_FS_GREP, receipt_status, &payload);
            }
        };

        if !base.exists() {
            let payload = HostFsGrepReceipt {
                status: "not_found".into(),
                matches: None,
                match_count: None,
                truncated: None,
                error_code: None,
                summary_text: None,
            };
            return build_receipt(
                intent,
                EffectKind::HOST_FS_GREP,
                ReceiptStatus::Ok,
                &payload,
            );
        }

        let max_results = params
            .max_results
            .unwrap_or(DEFAULT_GREP_MAX_RESULTS as u64)
            .min(usize::MAX as u64) as usize;

        let matches = match grep_collect(
            &base,
            &params.pattern,
            params.glob_filter.as_deref(),
            params.case_insensitive.unwrap_or(false),
        )
        .await
        {
            Ok(matches) => matches,
            Err(GrepCollectError::InvalidRegex(message)) => {
                let payload = HostFsGrepReceipt {
                    status: "invalid_regex".into(),
                    matches: None,
                    match_count: None,
                    truncated: None,
                    error_code: Some("invalid_regex".into()),
                    summary_text: Some(message),
                };
                return build_receipt(
                    intent,
                    EffectKind::HOST_FS_GREP,
                    ReceiptStatus::Ok,
                    &payload,
                );
            }
            Err(GrepCollectError::InvalidGlobFilter(message)) => {
                return fs_grep_error(intent, "invalid_glob_filter", message);
            }
            Err(GrepCollectError::Io(message)) => {
                return fs_grep_error(intent, "grep_failed", message);
            }
        };

        let total = matches.len();
        let mut text = String::new();
        for (idx, line) in matches.iter().take(max_results).enumerate() {
            if idx > 0 {
                text.push('\n');
            }
            text.push_str(line);
        }
        let truncated = total > max_results;

        let matches_payload =
            match materialize_text_output(self.store.as_ref(), mode, &text, self.output_cfg) {
                Ok(value) => value,
                Err(OutputMaterializeError::InlineRequiredTooLarge(_)) => {
                    return fs_grep_error(
                        intent,
                        "inline_required_too_large",
                        "grep output exceeds inline limit".into(),
                    );
                }
                Err(OutputMaterializeError::Store(err)) => {
                    return fs_grep_error(intent, "output_store_failed", err);
                }
            };

        let summary = if total == 0 {
            Some("No matches found".into())
        } else {
            Some(format!("{} match(es)", total))
        };

        let payload = HostFsGrepReceipt {
            status: "ok".into(),
            matches: matches_payload,
            match_count: Some(total as u64),
            truncated: Some(truncated),
            error_code: None,
            summary_text: summary,
        };
        build_receipt(
            intent,
            EffectKind::HOST_FS_GREP,
            ReceiptStatus::Ok,
            &payload,
        )
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for HostFsGlobAdapter<S> {
    fn kind(&self) -> &str {
        EffectKind::HOST_FS_GLOB
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostFsGlobParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fs_glob_error(intent, "invalid_params", err.to_string()),
        };

        let mode = params.output_mode.as_deref().unwrap_or("auto");
        if !output_mode_valid(mode) {
            return fs_glob_error(
                intent,
                "invalid_output_mode",
                format!("unsupported output_mode '{mode}'"),
            );
        }

        let session = match active_session(&self.state, &params.session_id).await {
            Ok(session) => session,
            Err(err) => return fs_glob_error(intent, err.code, err.message),
        };
        let base = match resolve_session_base(&session, params.path.as_deref()) {
            Ok(path) => path,
            Err(err) => {
                let payload = HostFsGlobReceipt {
                    status: if err.code == "forbidden" {
                        "forbidden"
                    } else {
                        "error"
                    }
                    .into(),
                    paths: None,
                    count: None,
                    truncated: None,
                    error_code: Some(err.code.into()),
                    summary_text: None,
                };
                let receipt_status = if err.code == "forbidden" {
                    ReceiptStatus::Ok
                } else {
                    ReceiptStatus::Error
                };
                return build_receipt(intent, EffectKind::HOST_FS_GLOB, receipt_status, &payload);
            }
        };

        if !base.exists() {
            let payload = HostFsGlobReceipt {
                status: "not_found".into(),
                paths: None,
                count: None,
                truncated: None,
                error_code: None,
                summary_text: None,
            };
            return build_receipt(
                intent,
                EffectKind::HOST_FS_GLOB,
                ReceiptStatus::Ok,
                &payload,
            );
        }

        let max_results = params
            .max_results
            .unwrap_or(DEFAULT_GLOB_MAX_RESULTS as u64)
            .min(usize::MAX as u64) as usize;

        let collected = match glob_collect(&base, &params.pattern) {
            Ok(values) => values,
            Err(GlobCollectError::InvalidPattern(message)) => {
                let payload = HostFsGlobReceipt {
                    status: "invalid_pattern".into(),
                    paths: None,
                    count: None,
                    truncated: None,
                    error_code: Some("invalid_pattern".into()),
                    summary_text: Some(message),
                };
                return build_receipt(
                    intent,
                    EffectKind::HOST_FS_GLOB,
                    ReceiptStatus::Ok,
                    &payload,
                );
            }
            Err(GlobCollectError::Io(message)) => {
                return fs_glob_error(intent, "glob_failed", message);
            }
        };

        let total = collected.len();
        let mut text = String::new();
        for (idx, value) in collected.iter().take(max_results).enumerate() {
            if idx > 0 {
                text.push('\n');
            }
            text.push_str(value);
        }
        let truncated = total > max_results;

        let paths_payload =
            match materialize_text_output(self.store.as_ref(), mode, &text, self.output_cfg) {
                Ok(value) => value,
                Err(OutputMaterializeError::InlineRequiredTooLarge(_)) => {
                    return fs_glob_error(
                        intent,
                        "inline_required_too_large",
                        "glob output exceeds inline limit".into(),
                    );
                }
                Err(OutputMaterializeError::Store(err)) => {
                    return fs_glob_error(intent, "output_store_failed", err);
                }
            };

        let summary = if total == 0 {
            Some("No files matched".into())
        } else {
            Some(format!("{} path(s) matched", total))
        };

        let payload = HostFsGlobReceipt {
            status: "ok".into(),
            paths: paths_payload,
            count: Some(total as u64),
            truncated: Some(truncated),
            error_code: None,
            summary_text: summary,
        };
        build_receipt(
            intent,
            EffectKind::HOST_FS_GLOB,
            ReceiptStatus::Ok,
            &payload,
        )
    }
}

#[async_trait]
impl AsyncEffectAdapter for HostFsStatAdapter {
    fn kind(&self) -> &str {
        EffectKind::HOST_FS_STAT
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostFsStatParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fs_stat_error(intent, "invalid_params", err.to_string()),
        };

        let session = match active_session(&self.state, &params.session_id).await {
            Ok(session) => session,
            Err(err) => return fs_stat_error(intent, err.code, err.message),
        };

        let path = match resolve_session_path(&session, &params.path) {
            Ok(path) => path,
            Err(err) => {
                let payload = HostFsStatReceipt {
                    status: if err.code == "forbidden" {
                        "forbidden".into()
                    } else {
                        "error".into()
                    },
                    exists: None,
                    is_dir: None,
                    size_bytes: None,
                    mtime_ns: None,
                    error_code: Some(err.code.into()),
                };
                let status = if err.code == "forbidden" {
                    ReceiptStatus::Ok
                } else {
                    ReceiptStatus::Error
                };
                return build_receipt(intent, EffectKind::HOST_FS_STAT, status, &payload);
            }
        };

        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let payload = HostFsStatReceipt {
                    status: "not_found".into(),
                    exists: Some(false),
                    is_dir: None,
                    size_bytes: None,
                    mtime_ns: None,
                    error_code: None,
                };
                return build_receipt(
                    intent,
                    EffectKind::HOST_FS_STAT,
                    ReceiptStatus::Ok,
                    &payload,
                );
            }
            Err(err) => return fs_stat_error(intent, "stat_failed", err.to_string()),
        };

        let mtime_ns = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos() as u64);
        let payload = HostFsStatReceipt {
            status: "ok".into(),
            exists: Some(true),
            is_dir: Some(metadata.is_dir()),
            size_bytes: Some(metadata.len()),
            mtime_ns,
            error_code: None,
        };
        build_receipt(
            intent,
            EffectKind::HOST_FS_STAT,
            ReceiptStatus::Ok,
            &payload,
        )
    }
}

#[async_trait]
impl AsyncEffectAdapter for HostFsExistsAdapter {
    fn kind(&self) -> &str {
        EffectKind::HOST_FS_EXISTS
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostFsExistsParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fs_exists_error(intent, "invalid_params", err.to_string()),
        };

        let session = match active_session(&self.state, &params.session_id).await {
            Ok(session) => session,
            Err(err) => return fs_exists_error(intent, err.code, err.message),
        };

        let path = match resolve_session_path(&session, &params.path) {
            Ok(path) => path,
            Err(err) => {
                let payload = HostFsExistsReceipt {
                    status: if err.code == "forbidden" {
                        "forbidden".into()
                    } else {
                        "error".into()
                    },
                    exists: None,
                    error_code: Some(err.code.into()),
                };
                let status = if err.code == "forbidden" {
                    ReceiptStatus::Ok
                } else {
                    ReceiptStatus::Error
                };
                return build_receipt(intent, EffectKind::HOST_FS_EXISTS, status, &payload);
            }
        };

        let exists = path.exists();
        let payload = HostFsExistsReceipt {
            status: "ok".into(),
            exists: Some(exists),
            error_code: None,
        };
        build_receipt(
            intent,
            EffectKind::HOST_FS_EXISTS,
            ReceiptStatus::Ok,
            &payload,
        )
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for HostFsListDirAdapter<S> {
    fn kind(&self) -> &str {
        EffectKind::HOST_FS_LIST_DIR
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostFsListDirParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fs_list_dir_error(intent, "invalid_params", err.to_string()),
        };

        let mode = params.output_mode.as_deref().unwrap_or("auto");
        if !output_mode_valid(mode) {
            return fs_list_dir_error(
                intent,
                "invalid_output_mode",
                format!("unsupported output_mode '{mode}'"),
            );
        }

        let session = match active_session(&self.state, &params.session_id).await {
            Ok(session) => session,
            Err(err) => return fs_list_dir_error(intent, err.code, err.message),
        };
        let base = match resolve_session_base(&session, params.path.as_deref()) {
            Ok(path) => path,
            Err(err) => {
                let payload = HostFsListDirReceipt {
                    status: if err.code == "forbidden" {
                        "forbidden".into()
                    } else {
                        "error".into()
                    },
                    entries: None,
                    count: None,
                    truncated: None,
                    error_code: Some(err.code.into()),
                    summary_text: None,
                };
                let status = if err.code == "forbidden" {
                    ReceiptStatus::Ok
                } else {
                    ReceiptStatus::Error
                };
                return build_receipt(intent, EffectKind::HOST_FS_LIST_DIR, status, &payload);
            }
        };

        let metadata = match tokio::fs::metadata(&base).await {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let payload = HostFsListDirReceipt {
                    status: "not_found".into(),
                    entries: None,
                    count: None,
                    truncated: None,
                    error_code: None,
                    summary_text: None,
                };
                return build_receipt(
                    intent,
                    EffectKind::HOST_FS_LIST_DIR,
                    ReceiptStatus::Ok,
                    &payload,
                );
            }
            Err(err) => return fs_list_dir_error(intent, "list_failed", err.to_string()),
        };
        if !metadata.is_dir() {
            return fs_list_dir_error(
                intent,
                "not_directory",
                format!("'{}' is not a directory", base.to_string_lossy()),
            );
        }

        let max_results = params.max_results.unwrap_or(100).min(usize::MAX as u64) as usize;
        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&base).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            let mut name = path
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string_lossy().to_string());
            if let Ok(meta) = entry.metadata().await {
                if meta.is_dir() {
                    name.push('/');
                }
            }
            entries.push(name);
        }
        entries.sort();
        let total = entries.len();
        let mut text = String::new();
        for (index, entry) in entries.iter().take(max_results).enumerate() {
            if index > 0 {
                text.push('\n');
            }
            text.push_str(entry);
        }
        let truncated = total > max_results;

        let entries_payload =
            match materialize_text_output(self.store.as_ref(), mode, &text, self.output_cfg) {
                Ok(value) => value,
                Err(OutputMaterializeError::InlineRequiredTooLarge(_)) => {
                    return fs_list_dir_error(
                        intent,
                        "inline_required_too_large",
                        "list_dir output exceeds inline limit".into(),
                    );
                }
                Err(OutputMaterializeError::Store(err)) => {
                    return fs_list_dir_error(intent, "output_store_failed", err);
                }
            };

        let payload = HostFsListDirReceipt {
            status: "ok".into(),
            entries: entries_payload,
            count: Some(total as u64),
            truncated: Some(truncated),
            error_code: None,
            summary_text: Some(format!(
                "{} entr{}",
                total,
                if total == 1 { "y" } else { "ies" }
            )),
        };
        build_receipt(
            intent,
            EffectKind::HOST_FS_LIST_DIR,
            ReceiptStatus::Ok,
            &payload,
        )
    }
}

enum WaitOutcome {
    Completed(Result<std::process::ExitStatus, std::io::Error>),
    TimedOut(Result<std::process::ExitStatus, std::io::Error>),
}

#[derive(Debug)]
struct SessionLookupError {
    code: &'static str,
    message: String,
}

async fn active_session(
    state: &Arc<Mutex<HostState>>,
    session_id: &str,
) -> Result<SessionRecord, SessionLookupError> {
    let mut guard = state.lock().await;
    let Some(session) = guard.sessions.get_mut(session_id) else {
        return Err(SessionLookupError {
            code: "session_not_found",
            message: format!("session '{session_id}' not found"),
        });
    };

    if session.closed {
        return Err(SessionLookupError {
            code: "session_closed",
            message: format!("session '{session_id}' is closed"),
        });
    }

    if let Some(expires_at_ns) = session.expires_at_ns {
        let now = now_wallclock_ns();
        if now > expires_at_ns {
            session.closed = true;
            session.ended_at_ns = Some(now);
            return Err(SessionLookupError {
                code: "session_expired",
                message: format!("session '{session_id}' expired at {expires_at_ns}"),
            });
        }
    }

    Ok(session.clone())
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

fn exec_error_receipt(
    intent: &EffectIntent,
    started_at_ns: u64,
    exit_code: i32,
    code: &str,
    message: String,
    stdout: Option<HostOutput>,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostExecReceipt {
        exit_code,
        status: "error".into(),
        stdout,
        stderr: None,
        started_at_ns,
        ended_at_ns: now_wallclock_ns(),
        error_code: Some(code.into()),
        error_message: Some(message),
    };
    build_receipt(
        intent,
        EffectKind::HOST_EXEC,
        ReceiptStatus::Error,
        &payload,
    )
}

fn fs_read_error(
    intent: &EffectIntent,
    code: &str,
    message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsReadFileReceipt {
        status: "error".into(),
        content: None,
        truncated: None,
        size_bytes: None,
        error_code: Some(code.into()),
    };
    let mut receipt = build_receipt(
        intent,
        EffectKind::HOST_FS_READ_FILE,
        ReceiptStatus::Error,
        &payload,
    )?;
    let _ = message;
    receipt.cost_cents = Some(0);
    Ok(receipt)
}

fn fs_write_error(
    intent: &EffectIntent,
    code: &str,
    _message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsWriteFileReceipt {
        status: "error".into(),
        written_bytes: None,
        created: None,
        new_mtime_ns: None,
        error_code: Some(code.into()),
    };
    build_receipt(
        intent,
        EffectKind::HOST_FS_WRITE_FILE,
        ReceiptStatus::Error,
        &payload,
    )
}

fn fs_edit_error(
    intent: &EffectIntent,
    code: &str,
    _message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsEditFileReceipt {
        status: "error".into(),
        replacements: None,
        applied: Some(false),
        summary_text: None,
        error_code: Some(code.into()),
    };
    build_receipt(
        intent,
        EffectKind::HOST_FS_EDIT_FILE,
        ReceiptStatus::Error,
        &payload,
    )
}

fn fs_apply_patch_error(
    intent: &EffectIntent,
    code: &str,
    _message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsApplyPatchReceipt {
        status: "error".into(),
        files_changed: None,
        changed_paths: None,
        ops: None,
        summary_text: None,
        errors: None,
        error_code: Some(code.into()),
    };
    build_receipt(
        intent,
        EffectKind::HOST_FS_APPLY_PATCH,
        ReceiptStatus::Error,
        &payload,
    )
}

fn fs_grep_error(
    intent: &EffectIntent,
    code: &str,
    _message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsGrepReceipt {
        status: "error".into(),
        matches: None,
        match_count: None,
        truncated: None,
        error_code: Some(code.into()),
        summary_text: None,
    };
    build_receipt(
        intent,
        EffectKind::HOST_FS_GREP,
        ReceiptStatus::Error,
        &payload,
    )
}

fn fs_glob_error(
    intent: &EffectIntent,
    code: &str,
    _message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsGlobReceipt {
        status: "error".into(),
        paths: None,
        count: None,
        truncated: None,
        error_code: Some(code.into()),
        summary_text: None,
    };
    build_receipt(
        intent,
        EffectKind::HOST_FS_GLOB,
        ReceiptStatus::Error,
        &payload,
    )
}

fn fs_stat_error(
    intent: &EffectIntent,
    code: &str,
    _message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsStatReceipt {
        status: "error".into(),
        exists: None,
        is_dir: None,
        size_bytes: None,
        mtime_ns: None,
        error_code: Some(code.into()),
    };
    build_receipt(
        intent,
        EffectKind::HOST_FS_STAT,
        ReceiptStatus::Error,
        &payload,
    )
}

fn fs_exists_error(
    intent: &EffectIntent,
    code: &str,
    _message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsExistsReceipt {
        status: "error".into(),
        exists: None,
        error_code: Some(code.into()),
    };
    build_receipt(
        intent,
        EffectKind::HOST_FS_EXISTS,
        ReceiptStatus::Error,
        &payload,
    )
}

fn fs_list_dir_error(
    intent: &EffectIntent,
    code: &str,
    _message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsListDirReceipt {
        status: "error".into(),
        entries: None,
        count: None,
        truncated: None,
        error_code: Some(code.into()),
        summary_text: None,
    };
    build_receipt(
        intent,
        EffectKind::HOST_FS_LIST_DIR,
        ReceiptStatus::Error,
        &payload,
    )
}

impl<S: Store> HostFsWriteFileAdapter<S> {
    fn resolve_file_content(&self, input: &HostFileContentInput) -> Result<Vec<u8>, String> {
        match input {
            HostFileContentInput::InlineText { inline_text } => {
                Ok(inline_text.text.as_bytes().to_vec())
            }
            HostFileContentInput::InlineBytes { inline_bytes } => Ok(inline_bytes.bytes.clone()),
            HostFileContentInput::BlobRef { blob_ref } => {
                let hash = Hash::from_hex_str(blob_ref.blob_ref.as_str())
                    .map_err(|err| err.to_string())?;
                self.store.get_blob(hash).map_err(|err| err.to_string())
            }
        }
    }
}

async fn write_file_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path '{}' has no parent", path.to_string_lossy()))?;
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| anyhow::anyhow!("invalid file name '{}')", path.to_string_lossy()))?;

    let tmp_name = format!(".{name}.aos-tmp-{}", now_wallclock_ns());
    let tmp_path = parent.join(tmp_name);

    tokio::fs::write(&tmp_path, bytes).await?;
    tokio::fs::rename(&tmp_path, path).await?;
    Ok(())
}

struct PatchApplyResult {
    files_changed: u64,
    changed_paths: Vec<String>,
    counts: PatchOpCounts,
}

struct PatchApplyError {
    status: String,
    error_code: String,
    message: String,
}

impl PatchApplyError {
    fn reject(code: &str, message: impl Into<String>) -> Self {
        Self {
            status: "reject".into(),
            error_code: code.into(),
            message: message.into(),
        }
    }

    fn not_found(code: &str, message: impl Into<String>) -> Self {
        Self {
            status: "not_found".into(),
            error_code: code.into(),
            message: message.into(),
        }
    }

    fn forbidden(code: &str, message: impl Into<String>) -> Self {
        Self {
            status: "forbidden".into(),
            error_code: code.into(),
            message: message.into(),
        }
    }

    fn error(code: &str, message: impl Into<String>) -> Self {
        Self {
            status: "error".into(),
            error_code: code.into(),
            message: message.into(),
        }
    }
}

async fn apply_patch_to_session(
    session: &SessionRecord,
    parsed: &ParsedPatch,
    dry_run: bool,
) -> Result<PatchApplyResult, PatchApplyError> {
    let mut staged: BTreeMap<PathBuf, Option<Vec<u8>>> = BTreeMap::new();
    let mut original: BTreeMap<PathBuf, Option<Vec<u8>>> = BTreeMap::new();

    for op in &parsed.operations {
        match op {
            PatchOperation::AddFile { path, lines } => {
                let resolved = resolve_patch_path(session, path)?;
                let current = read_staged_or_disk(&resolved, &staged, &mut original)
                    .await
                    .map_err(|err| PatchApplyError::error("read_failed", err.to_string()))?;
                if current.is_some() {
                    return Err(PatchApplyError::reject(
                        "add_target_exists",
                        format!("file '{}' already exists", path),
                    ));
                }
                staged.insert(resolved, Some(lines.join("\n").into_bytes()));
            }
            PatchOperation::DeleteFile { path } => {
                let resolved = resolve_patch_path(session, path)?;
                let current = read_staged_or_disk(&resolved, &staged, &mut original)
                    .await
                    .map_err(|err| PatchApplyError::error("read_failed", err.to_string()))?;
                if current.is_none() {
                    return Err(PatchApplyError::not_found(
                        "delete_target_not_found",
                        format!("file '{}' not found", path),
                    ));
                }
                staged.insert(resolved, None);
            }
            PatchOperation::UpdateFile {
                path,
                move_to,
                hunks,
            } => {
                let source = resolve_patch_path(session, path)?;
                let current = read_staged_or_disk(&source, &staged, &mut original)
                    .await
                    .map_err(|err| PatchApplyError::error("read_failed", err.to_string()))?;
                let Some(current_bytes) = current else {
                    return Err(PatchApplyError::not_found(
                        "update_target_not_found",
                        format!("file '{}' not found", path),
                    ));
                };
                let current_text = String::from_utf8(current_bytes).map_err(|_| {
                    PatchApplyError::reject(
                        "update_target_non_utf8",
                        format!("file '{}' is not utf8", path),
                    )
                })?;
                let updated_text = apply_update_hunks(&current_text, hunks).map_err(|err| {
                    PatchApplyError::reject("hunk_rejected", format!("{path}: {err}"))
                })?;

                let target = if let Some(move_to) = move_to {
                    resolve_patch_path(session, move_to)?
                } else {
                    source.clone()
                };

                if target != source {
                    let current_target = read_staged_or_disk(&target, &staged, &mut original)
                        .await
                        .map_err(|err| PatchApplyError::error("read_failed", err.to_string()))?;
                    if current_target.is_some() {
                        return Err(PatchApplyError::reject(
                            "move_target_exists",
                            format!(
                                "target '{}' already exists",
                                move_to.clone().unwrap_or_default()
                            ),
                        ));
                    }
                    staged.insert(source, None);
                }

                staged.insert(target, Some(updated_text.into_bytes()));
            }
        }
    }

    let mut changed_paths = Vec::new();
    for (path, next) in &staged {
        let prev = original.get(path).cloned().unwrap_or(None);
        if prev != *next {
            changed_paths.push(display_relative(&session.workdir, path));
        }
    }
    changed_paths.sort();

    if !dry_run {
        for (path, maybe_bytes) in &staged {
            match maybe_bytes {
                Some(bytes) => {
                    if let Some(parent) = path.parent() {
                        tokio::fs::create_dir_all(parent).await.map_err(|err| {
                            PatchApplyError::error("write_failed", err.to_string())
                        })?;
                    }
                    write_file_atomic(path, bytes)
                        .await
                        .map_err(|err| PatchApplyError::error("write_failed", err.to_string()))?;
                }
                None => {
                    if path.exists() {
                        tokio::fs::remove_file(path).await.map_err(|err| {
                            PatchApplyError::error("delete_failed", err.to_string())
                        })?;
                    }
                }
            }
        }
    }

    Ok(PatchApplyResult {
        files_changed: changed_paths.len() as u64,
        changed_paths,
        counts: parsed.counts.clone(),
    })
}

fn resolve_patch_path(session: &SessionRecord, raw_path: &str) -> Result<PathBuf, PatchApplyError> {
    resolve_session_path(session, raw_path).map_err(|err| map_path_error_to_patch(err, raw_path))
}

fn map_path_error_to_patch(error: PathResolveError, path: &str) -> PatchApplyError {
    match error.code {
        "forbidden" => PatchApplyError::forbidden(
            "path_forbidden",
            format!("path '{}' is outside session root", path),
        ),
        "invalid_path" => PatchApplyError::reject("invalid_path", error.message),
        _ => PatchApplyError::error("path_resolution_failed", error.message),
    }
}

async fn read_staged_or_disk(
    path: &Path,
    staged: &BTreeMap<PathBuf, Option<Vec<u8>>>,
    original: &mut BTreeMap<PathBuf, Option<Vec<u8>>>,
) -> std::io::Result<Option<Vec<u8>>> {
    if let Some(value) = staged.get(path) {
        return Ok(value.clone());
    }
    if let Some(value) = original.get(path) {
        return Ok(value.clone());
    }
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            original.insert(path.to_path_buf(), Some(bytes.clone()));
            Ok(Some(bytes))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            original.insert(path.to_path_buf(), None);
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

fn resolve_patch_text<S: Store>(
    store: &S,
    params: &HostFsApplyPatchParams,
) -> Result<String, String> {
    let bytes = match &params.patch {
        aos_effects::builtins::HostPatchInput::InlineText { inline_text } => {
            inline_text.text.as_bytes().to_vec()
        }
        aos_effects::builtins::HostPatchInput::BlobRef { blob_ref } => {
            let hash =
                Hash::from_hex_str(blob_ref.blob_ref.as_str()).map_err(|err| err.to_string())?;
            store.get_blob(hash).map_err(|err| err.to_string())?
        }
    };

    String::from_utf8(bytes).map_err(|_| "patch must be valid utf8".into())
}

enum GrepCollectError {
    InvalidRegex(String),
    InvalidGlobFilter(String),
    Io(String),
}

async fn grep_collect(
    base: &Path,
    pattern: &str,
    glob_filter: Option<&str>,
    case_insensitive: bool,
) -> Result<Vec<String>, GrepCollectError> {
    if rg_available().await {
        match grep_collect_rg(base, pattern, glob_filter, case_insensitive).await {
            Ok(lines) => return Ok(lines),
            Err(GrepCollectError::Io(_)) => {
                // fallback below
            }
            Err(err) => return Err(err),
        }
    }
    grep_collect_native(base, pattern, glob_filter, case_insensitive)
}

async fn rg_available() -> bool {
    Command::new("rg")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|status| status.success())
        .unwrap_or(false)
}

async fn grep_collect_rg(
    base: &Path,
    pattern: &str,
    glob_filter: Option<&str>,
    case_insensitive: bool,
) -> Result<Vec<String>, GrepCollectError> {
    let (cwd, target) = if base.is_dir() {
        (base.to_path_buf(), PathBuf::from("."))
    } else {
        let parent = base
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let file = base
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        (parent, file)
    };

    let mut cmd = Command::new("rg");
    cmd.current_dir(cwd)
        .arg("--line-number")
        .arg("--with-filename")
        .arg("--color")
        .arg("never")
        .arg("--no-heading")
        .arg("--sort")
        .arg("path");

    if case_insensitive {
        cmd.arg("-i");
    }
    if let Some(glob) = glob_filter {
        cmd.arg("-g").arg(glob);
    }
    cmd.arg(pattern).arg(target);

    let output = cmd
        .output()
        .await
        .map_err(|err| GrepCollectError::Io(err.to_string()))?;

    match output.status.code() {
        Some(0) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(stdout
                .lines()
                .map(|line| line.trim_end().to_string())
                .filter(|line| !line.is_empty())
                .collect())
        }
        Some(1) => Ok(Vec::new()),
        Some(2) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(GrepCollectError::InvalidRegex(stderr.trim().to_string()))
        }
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(GrepCollectError::Io(stderr.trim().to_string()))
        }
    }
}

fn grep_collect_native(
    base: &Path,
    pattern: &str,
    glob_filter: Option<&str>,
    case_insensitive: bool,
) -> Result<Vec<String>, GrepCollectError> {
    let regex = RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .build()
        .map_err(|err| GrepCollectError::InvalidRegex(err.to_string()))?;

    let matcher = if let Some(filter) = glob_filter {
        Some(
            Glob::new(filter)
                .map_err(|err| GrepCollectError::InvalidGlobFilter(err.to_string()))?
                .compile_matcher(),
        )
    } else {
        None
    };

    let mut files = Vec::new();
    if base.is_file() {
        files.push(base.to_path_buf());
    } else {
        for entry in WalkDir::new(base).follow_links(false).into_iter() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => return Err(GrepCollectError::Io(err.to_string())),
            };
            if !entry.file_type().is_file() {
                continue;
            }
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();

    let mut lines_out = Vec::new();
    for file in files {
        let relative = if base.is_file() {
            file.file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| file.to_string_lossy().to_string())
        } else {
            display_relative(base, &file)
        };
        if let Some(matcher) = &matcher {
            if !matcher.is_match(Path::new(&relative)) {
                continue;
            }
        }

        let bytes = std::fs::read(&file).map_err(|err| GrepCollectError::Io(err.to_string()))?;
        let content = String::from_utf8_lossy(&bytes);
        for (line_no, line) in content.lines().enumerate() {
            if regex.is_match(line) {
                lines_out.push(format!("{}:{}:{}", relative, line_no + 1, line));
            }
        }
    }

    Ok(lines_out)
}

enum GlobCollectError {
    InvalidPattern(String),
    Io(String),
}

fn glob_collect(base: &Path, pattern: &str) -> Result<Vec<String>, GlobCollectError> {
    let matcher = Glob::new(pattern)
        .map_err(|err| GlobCollectError::InvalidPattern(err.to_string()))?
        .compile_matcher();

    let mut entries: Vec<(String, u64)> = Vec::new();
    if base.is_file() {
        let relative = base
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| base.to_string_lossy().to_string());
        if matcher.is_match(Path::new(&relative)) {
            let mtime = file_mtime_ns(base)?;
            entries.push((relative, mtime));
        }
    } else {
        for entry in WalkDir::new(base).follow_links(false).into_iter() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => return Err(GlobCollectError::Io(err.to_string())),
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let relative = display_relative(base, entry.path());
            if matcher.is_match(Path::new(&relative)) {
                let mtime = file_mtime_ns(entry.path())?;
                entries.push((relative, mtime));
            }
        }
    }

    entries.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    Ok(entries.into_iter().map(|(path, _)| path).collect())
}

fn file_mtime_ns(path: &Path) -> Result<u64, GlobCollectError> {
    let metadata = std::fs::metadata(path).map_err(|err| GlobCollectError::Io(err.to_string()))?;
    let modified = metadata
        .modified()
        .map_err(|err| GlobCollectError::Io(err.to_string()))?;
    Ok(modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::{builtins::builtin_schemas, schema_index::SchemaIndex};
    use aos_effects::builtins::{
        HostFsReadFileParams, HostFsWriteFileParams, HostLocalTarget, HostTarget,
    };
    use aos_store::MemStore;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn intent_for<T: serde::Serialize>(kind: &str, params: &T, seed: u8) -> EffectIntent {
        EffectIntent::from_raw_params(
            EffectKind::new(kind),
            "cap_host",
            serde_cbor::to_vec(params).expect("encode params"),
            [seed; 32],
        )
        .expect("intent")
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

    fn open_params(workdir: &str) -> HostSessionOpenParams {
        HostSessionOpenParams {
            target: HostTarget {
                local: Some(HostLocalTarget {
                    mounts: None,
                    workdir: Some(workdir.into()),
                    env: None,
                    network_mode: "none".into(),
                }),
            },
            session_ttl_ns: None,
            labels: None,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn process_session_open_exec_signal_roundtrip() {
        let store = Arc::new(MemStore::new());
        let (open_adapter, exec_adapter, signal_adapter) = make_host_adapters(store);

        let open_receipt = open_adapter
            .execute(&intent_for(
                EffectKind::HOST_SESSION_OPEN,
                &open_params("."),
                1,
            ))
            .await
            .expect("open receipt");
        assert_eq!(open_receipt.status, ReceiptStatus::Ok);
        assert_schema_normalizes("sys/HostSessionOpenReceipt@1", &open_receipt.payload_cbor);
        let open_payload: HostSessionOpenReceipt =
            serde_cbor::from_slice(&open_receipt.payload_cbor).expect("decode open payload");

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
    }

    #[tokio::test(flavor = "current_thread")]
    async fn process_host_fs_read_and_write_roundtrip() {
        let tmp = TempDir::new().expect("temp dir");
        let store = Arc::new(MemStore::new());
        let set = make_host_adapter_set(store);

        let open_receipt = set
            .session_open
            .execute(&intent_for(
                EffectKind::HOST_SESSION_OPEN,
                &open_params(tmp.path().to_string_lossy().as_ref()),
                9,
            ))
            .await
            .expect("open receipt");
        let open_payload: HostSessionOpenReceipt =
            serde_cbor::from_slice(&open_receipt.payload_cbor).expect("decode open payload");

        let write_params = HostFsWriteFileParams {
            session_id: open_payload.session_id.clone(),
            path: "src/lib.rs".into(),
            content: HostFileContentInput::InlineText {
                inline_text: aos_effects::builtins::HostInlineText {
                    text: "fn main() {}\n".into(),
                },
            },
            create_parents: Some(true),
            mode: Some("overwrite".into()),
        };
        let write_receipt = set
            .fs_write_file
            .execute(&intent_for(
                EffectKind::HOST_FS_WRITE_FILE,
                &write_params,
                10,
            ))
            .await
            .expect("write receipt");
        assert_eq!(write_receipt.status, ReceiptStatus::Ok);
        assert_schema_normalizes("sys/HostFsWriteFileReceipt@1", &write_receipt.payload_cbor);

        let read_params = HostFsReadFileParams {
            session_id: open_payload.session_id,
            path: "src/lib.rs".into(),
            offset_bytes: None,
            max_bytes: None,
            encoding: Some("utf8".into()),
            output_mode: Some("require_inline".into()),
        };
        let read_receipt = set
            .fs_read_file
            .execute(&intent_for(EffectKind::HOST_FS_READ_FILE, &read_params, 11))
            .await
            .expect("read receipt");
        assert_eq!(read_receipt.status, ReceiptStatus::Ok);
        assert_schema_normalizes("sys/HostFsReadFileReceipt@1", &read_receipt.payload_cbor);
    }

    #[test]
    fn edit_fuzzy_replaces_quotes_and_whitespace() {
        let source = "let s = \"hello   world\";\n";
        let out = apply_edit(source, "let s = “hello world”;", "let s = \"ok\";", false)
            .expect("expected applied");
        assert_eq!(out.updated, "let s = \"ok\";\n");
        assert_eq!(out.replacements, 1);
    }
}
