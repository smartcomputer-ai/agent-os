use std::sync::Arc;

use aos_cbor::Hash;
use aos_effects::builtins::{
    HostExecParams, HostExecProgressFrame, HostExecReceipt, HostFsApplyPatchReceipt,
    HostFsEditFileParams, HostFsEditFileReceipt, HostFsExistsParams, HostFsExistsReceipt,
    HostFsGlobParams, HostFsGlobReceipt, HostFsGrepParams, HostFsGrepReceipt, HostFsListDirParams,
    HostFsListDirReceipt, HostFsReadFileParams, HostFsReadFileReceipt, HostFsStatParams,
    HostFsStatReceipt, HostFsWriteFileReceipt, HostOutput, HostPatchOpsSummary, HostSandboxTarget,
    HostSessionOpenParams, HostSessionOpenReceipt, HostSessionSignalParams,
    HostSessionSignalReceipt,
};
use aos_effects::{EffectIntent, EffectReceipt, EffectStreamFrame, ReceiptStatus};
use aos_kernel::Store;
use async_trait::async_trait;
use fabric_client::{
    ExecProgress, ExecTerminalStatus, FabricClientError, FabricControllerClient,
    collect_exec_with_progress,
};
use fabric_protocol::{
    CloseSignal, ControllerExecRequest, ControllerSessionOpenRequest, ControllerSessionStatus,
    ControllerSignalSessionRequest, FabricBytes, FabricSandboxTarget, FabricSessionSignal,
    FabricSessionTarget, FsApplyPatchRequest, FsEditFileRequest, FsEntryKind, FsFileWriteRequest,
    FsGlobRequest, FsGrepMatch, FsGrepRequest, FsPathQuery, MountSpec, NetworkMode, QuiesceSignal,
    RequestId, ResourceLimits, ResumeSignal, SessionId,
};

use crate::config::FabricAdapterConfig;

use super::super::traits::{
    AdapterStartContext, AsyncEffectAdapter, EffectUpdate, EffectUpdateSender,
};
use super::output::{
    OutputConfig, OutputMaterializeError, materialize_binary_output, materialize_output,
    materialize_text_output, output_mode_valid,
};
use super::shared::{
    build_receipt, decode_host_fs_apply_patch_params, decode_host_fs_write_file_params,
    now_wallclock_ns, resolve_file_content, resolve_patch_text,
};

const HOST_SESSION_OPEN_FABRIC: &str = "host.session.open.fabric";
const HOST_EXEC_FABRIC: &str = "host.exec.fabric";
const HOST_SESSION_SIGNAL_FABRIC: &str = "host.session.signal.fabric";
const HOST_FS_READ_FILE_FABRIC: &str = "host.fs.read_file.fabric";
const HOST_FS_WRITE_FILE_FABRIC: &str = "host.fs.write_file.fabric";
const HOST_FS_EDIT_FILE_FABRIC: &str = "host.fs.edit_file.fabric";
const HOST_FS_APPLY_PATCH_FABRIC: &str = "host.fs.apply_patch.fabric";
const HOST_FS_GREP_FABRIC: &str = "host.fs.grep.fabric";
const HOST_FS_GLOB_FABRIC: &str = "host.fs.glob.fabric";
const HOST_FS_STAT_FABRIC: &str = "host.fs.stat.fabric";
const HOST_FS_EXISTS_FABRIC: &str = "host.fs.exists.fabric";
const HOST_FS_LIST_DIR_FABRIC: &str = "host.fs.list_dir.fabric";

pub struct FabricHostAdapterSet<S: Store> {
    pub session_open: FabricHostSessionOpenAdapter,
    pub exec: FabricHostExecAdapter<S>,
    pub session_signal: FabricHostSessionSignalAdapter,
    pub fs_read_file: FabricHostFsReadFileAdapter<S>,
    pub fs_write_file: FabricHostFsWriteFileAdapter<S>,
    pub fs_edit_file: FabricHostFsEditFileAdapter,
    pub fs_apply_patch: FabricHostFsApplyPatchAdapter<S>,
    pub fs_grep: FabricHostFsGrepAdapter<S>,
    pub fs_glob: FabricHostFsGlobAdapter<S>,
    pub fs_stat: FabricHostFsStatAdapter,
    pub fs_exists: FabricHostFsExistsAdapter,
    pub fs_list_dir: FabricHostFsListDirAdapter<S>,
}

#[derive(Clone)]
struct FabricHostShared {
    client: FabricControllerClient,
    config: FabricAdapterConfig,
}

pub struct FabricHostSessionOpenAdapter {
    shared: FabricHostShared,
}

pub struct FabricHostExecAdapter<S: Store> {
    shared: FabricHostShared,
    store: Arc<S>,
    output_cfg: OutputConfig,
}

pub struct FabricHostSessionSignalAdapter {
    shared: FabricHostShared,
}

pub struct FabricHostFsReadFileAdapter<S: Store> {
    shared: FabricHostShared,
    store: Arc<S>,
    output_cfg: OutputConfig,
}

pub struct FabricHostFsWriteFileAdapter<S: Store> {
    shared: FabricHostShared,
    store: Arc<S>,
}

pub struct FabricHostFsEditFileAdapter {
    shared: FabricHostShared,
}

pub struct FabricHostFsApplyPatchAdapter<S: Store> {
    shared: FabricHostShared,
    store: Arc<S>,
}

pub struct FabricHostFsGrepAdapter<S: Store> {
    shared: FabricHostShared,
    store: Arc<S>,
    output_cfg: OutputConfig,
}

pub struct FabricHostFsGlobAdapter<S: Store> {
    shared: FabricHostShared,
    store: Arc<S>,
    output_cfg: OutputConfig,
}

pub struct FabricHostFsStatAdapter {
    shared: FabricHostShared,
}

pub struct FabricHostFsExistsAdapter {
    shared: FabricHostShared,
}

pub struct FabricHostFsListDirAdapter<S: Store> {
    shared: FabricHostShared,
    store: Arc<S>,
    output_cfg: OutputConfig,
}

pub fn make_fabric_host_adapter_set<S: Store + Send + Sync + 'static>(
    store: Arc<S>,
    config: FabricAdapterConfig,
) -> FabricHostAdapterSet<S> {
    let mut client = FabricControllerClient::new(config.controller_url.clone());
    if let Some(token) = &config.bearer_token {
        client = client.with_bearer_token(token.clone());
    }
    let shared = FabricHostShared { client, config };
    let output_cfg = OutputConfig::default();
    FabricHostAdapterSet {
        session_open: FabricHostSessionOpenAdapter {
            shared: shared.clone(),
        },
        exec: FabricHostExecAdapter {
            shared: shared.clone(),
            store: store.clone(),
            output_cfg,
        },
        session_signal: FabricHostSessionSignalAdapter {
            shared: shared.clone(),
        },
        fs_read_file: FabricHostFsReadFileAdapter {
            shared: shared.clone(),
            store: store.clone(),
            output_cfg,
        },
        fs_write_file: FabricHostFsWriteFileAdapter {
            shared: shared.clone(),
            store: store.clone(),
        },
        fs_edit_file: FabricHostFsEditFileAdapter {
            shared: shared.clone(),
        },
        fs_apply_patch: FabricHostFsApplyPatchAdapter {
            shared: shared.clone(),
            store: store.clone(),
        },
        fs_grep: FabricHostFsGrepAdapter {
            shared: shared.clone(),
            store: store.clone(),
            output_cfg,
        },
        fs_glob: FabricHostFsGlobAdapter {
            shared: shared.clone(),
            store: store.clone(),
            output_cfg,
        },
        fs_stat: FabricHostFsStatAdapter {
            shared: shared.clone(),
        },
        fs_exists: FabricHostFsExistsAdapter {
            shared: shared.clone(),
        },
        fs_list_dir: FabricHostFsListDirAdapter {
            shared,
            store,
            output_cfg,
        },
    }
}

#[async_trait]
impl AsyncEffectAdapter for FabricHostSessionOpenAdapter {
    fn kind(&self) -> &str {
        HOST_SESSION_OPEN_FABRIC
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let started_at_ns = now_wallclock_ns();
        let params: HostSessionOpenParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => {
                return fabric_session_open_error(
                    intent,
                    started_at_ns,
                    "invalid_params",
                    err.to_string(),
                );
            }
        };
        let Some(target) = params.target.as_sandbox() else {
            return fabric_session_open_error(
                intent,
                started_at_ns,
                "unsupported_target",
                "Fabric host adapter only supports sandbox targets".to_string(),
            );
        };

        let request = match controller_open_request(intent, &self.shared.config, &params, target) {
            Ok(request) => request,
            Err(message) => {
                return fabric_session_open_error(intent, started_at_ns, "invalid_target", message);
            }
        };

        let opened = match self.shared.client.open_session(&request).await {
            Ok(opened) => opened,
            Err(err) => {
                return fabric_session_open_error(
                    intent,
                    started_at_ns,
                    fabric_error_code(&err),
                    err.to_string(),
                );
            }
        };

        let payload = HostSessionOpenReceipt {
            session_id: opened.session_id.0,
            status: controller_session_status(opened.status).to_string(),
            started_at_ns: u128_to_u64(opened.created_at_ns),
            expires_at_ns: opened.expires_at_ns.map(u128_to_u64),
            error_code: None,
            error_message: None,
        };
        build_receipt(
            intent,
            HOST_SESSION_OPEN_FABRIC,
            ReceiptStatus::Ok,
            &payload,
        )
    }
}

impl<S: Store + Send + Sync + 'static> FabricHostExecAdapter<S> {
    async fn run_exec(
        &self,
        intent: &EffectIntent,
        context: Option<AdapterStartContext>,
        updates: Option<EffectUpdateSender>,
    ) -> anyhow::Result<EffectReceipt> {
        let started_at_ns = now_wallclock_ns();
        let params: HostExecParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => {
                return fabric_exec_error(
                    intent,
                    started_at_ns,
                    -1,
                    "invalid_params",
                    err.to_string(),
                    None,
                );
            }
        };

        if params.argv.is_empty() {
            return fabric_exec_error(
                intent,
                started_at_ns,
                -1,
                "argv_empty",
                "argv must not be empty".to_string(),
                None,
            );
        }
        let mode = params.output_mode.as_deref().unwrap_or("auto");
        if !output_mode_valid(mode) {
            return fabric_exec_error(
                intent,
                started_at_ns,
                -1,
                "invalid_output_mode",
                format!("unsupported output_mode '{mode}'"),
                None,
            );
        }

        let stdin = match params.stdin_ref.as_ref() {
            Some(stdin_ref) => match read_store_blob(self.store.as_ref(), stdin_ref.as_str()) {
                Ok(bytes) => Some(FabricBytes::from_bytes_auto(bytes).into()),
                Err(err) => {
                    return fabric_exec_error(
                        intent,
                        started_at_ns,
                        -1,
                        "invalid_stdin_ref",
                        err,
                        None,
                    );
                }
            },
            None => None,
        };
        let request = ControllerExecRequest {
            request_id: Some(request_id(intent)),
            argv: params.argv,
            cwd: params.cwd,
            env_patch: params.env_patch.unwrap_or_default(),
            stdin,
            timeout_ns: params.timeout_ns.map(u128::from),
        };

        let stream = match self
            .shared
            .client
            .exec_session_stream(&SessionId(params.session_id), &request)
            .await
        {
            Ok(stream) => stream,
            Err(err) => {
                return fabric_exec_error(
                    intent,
                    started_at_ns,
                    -1,
                    fabric_error_code(&err),
                    err.to_string(),
                    None,
                );
            }
        };
        let mut progress_seq = 0_u64;
        let transcript = match collect_exec_with_progress(
            stream,
            self.shared.config.exec_progress_interval,
            |progress| {
                let (Some(context), Some(updates)) = (&context, &updates) else {
                    return;
                };
                progress_seq = progress_seq.saturating_add(1);
                let Ok(frame) = fabric_exec_progress_frame(intent, context, progress_seq, progress)
                else {
                    return;
                };
                let _ = updates.try_send(EffectUpdate::StreamFrame(frame));
            },
        )
        .await
        {
            Ok(transcript) => transcript,
            Err(err) => {
                return fabric_exec_error(
                    intent,
                    started_at_ns,
                    -1,
                    fabric_error_code(&err),
                    err.to_string(),
                    None,
                );
            }
        };

        let exit_code = transcript.exit_code.unwrap_or(-1);
        let stdout = match materialize_output(
            self.store.as_ref(),
            mode,
            &transcript.stdout,
            self.output_cfg,
        ) {
            Ok(output) => output,
            Err(err) => {
                return fabric_exec_error(
                    intent,
                    started_at_ns,
                    exit_code,
                    "output_store_failed",
                    output_error_message(err, "stdout", self.output_cfg),
                    None,
                );
            }
        };
        let stderr = match materialize_output(
            self.store.as_ref(),
            mode,
            &transcript.stderr,
            self.output_cfg,
        ) {
            Ok(output) => output,
            Err(err) => {
                return fabric_exec_error(
                    intent,
                    started_at_ns,
                    exit_code,
                    "output_store_failed",
                    output_error_message(err, "stderr", self.output_cfg),
                    stdout,
                );
            }
        };

        let ended_at_ns = now_wallclock_ns();
        let (status, receipt_status, error_code, error_message) = match transcript.terminal_status {
            ExecTerminalStatus::Exited => ("ok", ReceiptStatus::Ok, None, None),
            ExecTerminalStatus::Error => (
                "error",
                ReceiptStatus::Error,
                Some("fabric_exec_error".to_string()),
                transcript.error_message,
            ),
            ExecTerminalStatus::StreamEnded => (
                "error",
                ReceiptStatus::Error,
                Some("fabric_exec_stream_ended".to_string()),
                Some("Fabric exec stream ended without an exit event".to_string()),
            ),
        };
        let payload = HostExecReceipt {
            exit_code,
            status: status.to_string(),
            stdout,
            stderr,
            started_at_ns,
            ended_at_ns,
            error_code,
            error_message,
        };
        build_receipt(intent, HOST_EXEC_FABRIC, receipt_status, &payload)
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for FabricHostExecAdapter<S> {
    fn kind(&self) -> &str {
        HOST_EXEC_FABRIC
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        self.run_exec(intent, None, None).await
    }

    async fn ensure_started_with_context(
        &self,
        intent: EffectIntent,
        context: Option<AdapterStartContext>,
        updates: EffectUpdateSender,
    ) -> anyhow::Result<()> {
        let receipt = self
            .run_exec(&intent, context, Some(updates.clone()))
            .await?;
        updates
            .send(EffectUpdate::Receipt(receipt))
            .await
            .map_err(|_| anyhow::anyhow!("effect update receiver dropped"))?;
        Ok(())
    }
}

#[async_trait]
impl AsyncEffectAdapter for FabricHostSessionSignalAdapter {
    fn kind(&self) -> &str {
        HOST_SESSION_SIGNAL_FABRIC
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostSessionSignalParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fabric_signal_error(intent, "invalid_params", err.to_string()),
        };
        let signal = match fabric_signal(&params.signal) {
            Ok(signal) => signal,
            Err(message) => return fabric_signal_error(intent, "invalid_signal", message),
        };
        let request = ControllerSignalSessionRequest {
            request_id: None,
            signal,
        };
        let summary = match self
            .shared
            .client
            .signal_session(&SessionId(params.session_id), &request)
            .await
        {
            Ok(summary) => summary,
            Err(err) => {
                return fabric_signal_error(intent, fabric_error_code(&err), err.to_string());
            }
        };
        let payload = HostSessionSignalReceipt {
            status: controller_session_status(summary.status).to_string(),
            exit_code: None,
            ended_at_ns: summary.closed_at_ns.map(u128_to_u64),
            error_code: None,
            error_message: None,
        };
        build_receipt(
            intent,
            HOST_SESSION_SIGNAL_FABRIC,
            ReceiptStatus::Ok,
            &payload,
        )
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for FabricHostFsReadFileAdapter<S> {
    fn kind(&self) -> &str {
        HOST_FS_READ_FILE_FABRIC
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostFsReadFileParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fabric_fs_read_error(intent, "invalid_params", err.to_string()),
        };
        let mode = params.output_mode.as_deref().unwrap_or("auto");
        if !output_mode_valid(mode) {
            return fabric_fs_read_error(
                intent,
                "invalid_output_mode",
                format!("unsupported output_mode '{mode}'"),
            );
        }
        let response = match self
            .shared
            .client
            .read_file(
                &SessionId(params.session_id),
                &FsPathQuery {
                    path: params.path,
                    offset_bytes: params.offset_bytes,
                    max_bytes: params.max_bytes,
                },
            )
            .await
        {
            Ok(response) => response,
            Err(err) => {
                return fabric_fs_read_error(intent, fabric_error_code(&err), err.to_string());
            }
        };
        let bytes = match response.content.decode_bytes() {
            Ok(bytes) => bytes,
            Err(err) => return fabric_fs_read_error(intent, "invalid_payload", err),
        };
        let encoding = params.encoding.as_deref().unwrap_or("utf8");
        let content = match encoding {
            "bytes" => {
                match materialize_binary_output(self.store.as_ref(), mode, &bytes, self.output_cfg)
                {
                    Ok(output) => output,
                    Err(err) => {
                        return fabric_fs_read_error(
                            intent,
                            "output_store_failed",
                            output_error_message(err, "content", self.output_cfg),
                        );
                    }
                }
            }
            "utf8" => {
                if let Err(err) = std::str::from_utf8(&bytes) {
                    return fabric_fs_read_error(intent, "invalid_utf8", err.to_string());
                }
                match materialize_output(self.store.as_ref(), mode, &bytes, self.output_cfg) {
                    Ok(output) => output,
                    Err(err) => {
                        return fabric_fs_read_error(
                            intent,
                            "output_store_failed",
                            output_error_message(err, "content", self.output_cfg),
                        );
                    }
                }
            }
            other => {
                return fabric_fs_read_error(
                    intent,
                    "invalid_encoding",
                    format!("unsupported encoding '{other}'"),
                );
            }
        };
        let payload = HostFsReadFileReceipt {
            status: "ok".to_string(),
            content,
            truncated: Some(response.truncated),
            size_bytes: Some(response.size_bytes),
            mtime_ns: response.mtime_ns.map(u128_to_u64),
            error_code: None,
            error_message: None,
        };
        build_receipt(
            intent,
            HOST_FS_READ_FILE_FABRIC,
            ReceiptStatus::Ok,
            &payload,
        )
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for FabricHostFsWriteFileAdapter<S> {
    fn kind(&self) -> &str {
        HOST_FS_WRITE_FILE_FABRIC
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params = match decode_host_fs_write_file_params(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fabric_fs_write_error(intent, "invalid_params", err.to_string()),
        };
        let mode = params.mode.as_deref().unwrap_or("overwrite");
        if mode != "overwrite" && mode != "create_new" {
            return fabric_fs_write_error(
                intent,
                "invalid_mode",
                format!("unsupported mode '{mode}'"),
            );
        }
        if mode == "create_new" {
            match self
                .shared
                .client
                .exists(
                    &SessionId(params.session_id.clone()),
                    &FsPathQuery {
                        path: params.path.clone(),
                        offset_bytes: None,
                        max_bytes: None,
                    },
                )
                .await
            {
                Ok(response) if response.exists => {
                    let payload = HostFsWriteFileReceipt {
                        status: "already_exists".to_string(),
                        written_bytes: None,
                        created: Some(false),
                        new_mtime_ns: None,
                        error_code: None,
                    };
                    return build_receipt(
                        intent,
                        HOST_FS_WRITE_FILE_FABRIC,
                        ReceiptStatus::Ok,
                        &payload,
                    );
                }
                Ok(_) => {}
                Err(err) => {
                    return fabric_fs_write_error(intent, fabric_error_code(&err), err.to_string());
                }
            }
        }

        let bytes = match resolve_file_content(self.store.as_ref(), &params.content) {
            Ok(bytes) => bytes,
            Err(err) => return fabric_fs_write_error(intent, "invalid_content", err),
        };
        let session_id = params.session_id;
        let path = params.path;
        let response = match self
            .shared
            .client
            .write_file(
                &SessionId(session_id.clone()),
                &FsFileWriteRequest {
                    path: path.clone(),
                    content: FabricBytes::from_bytes_auto(bytes),
                    create_parents: params.create_parents.unwrap_or(false),
                },
            )
            .await
        {
            Ok(response) => response,
            Err(err) => {
                return fabric_fs_write_error(intent, fabric_error_code(&err), err.to_string());
            }
        };
        let new_mtime_ns = match self
            .shared
            .client
            .stat(
                &SessionId(session_id),
                &FsPathQuery {
                    path,
                    offset_bytes: None,
                    max_bytes: None,
                },
            )
            .await
        {
            Ok(stat) => stat.mtime_ns.map(u128_to_u64),
            Err(_) => None,
        };
        let payload = HostFsWriteFileReceipt {
            status: "ok".to_string(),
            written_bytes: Some(response.bytes_written),
            created: None,
            new_mtime_ns,
            error_code: None,
        };
        build_receipt(
            intent,
            HOST_FS_WRITE_FILE_FABRIC,
            ReceiptStatus::Ok,
            &payload,
        )
    }
}

#[async_trait]
impl AsyncEffectAdapter for FabricHostFsEditFileAdapter {
    fn kind(&self) -> &str {
        HOST_FS_EDIT_FILE_FABRIC
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostFsEditFileParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fabric_fs_edit_error(intent, "invalid_params", err.to_string()),
        };
        if params.old_string.is_empty() {
            return fabric_fs_edit_error(
                intent,
                "invalid_input_empty_old_string",
                "old_string must not be empty".to_string(),
            );
        }

        let response = match self
            .shared
            .client
            .edit_file(
                &SessionId(params.session_id),
                &FsEditFileRequest {
                    path: params.path,
                    old_string: params.old_string,
                    new_string: params.new_string,
                    replace_all: params.replace_all.unwrap_or(false),
                },
            )
            .await
        {
            Ok(response) => response,
            Err(err) if fabric_error_code(&err) == "not_found" => {
                let payload = HostFsEditFileReceipt {
                    status: "not_found".to_string(),
                    replacements: None,
                    applied: Some(false),
                    summary_text: None,
                    error_code: Some("path_not_found".to_string()),
                };
                return build_receipt(
                    intent,
                    HOST_FS_EDIT_FILE_FABRIC,
                    ReceiptStatus::Ok,
                    &payload,
                );
            }
            Err(err) => {
                return fabric_fs_edit_error(intent, fabric_error_code(&err), err.to_string());
            }
        };

        let payload = HostFsEditFileReceipt {
            status: "ok".to_string(),
            replacements: Some(response.replacements),
            applied: Some(response.applied),
            summary_text: Some(format!(
                "Updated {} ({} replacement{})",
                response.path,
                response.replacements,
                if response.replacements == 1 { "" } else { "s" }
            )),
            error_code: None,
        };
        build_receipt(
            intent,
            HOST_FS_EDIT_FILE_FABRIC,
            ReceiptStatus::Ok,
            &payload,
        )
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for FabricHostFsApplyPatchAdapter<S> {
    fn kind(&self) -> &str {
        HOST_FS_APPLY_PATCH_FABRIC
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params = match decode_host_fs_apply_patch_params(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => {
                return fabric_fs_apply_patch_error(intent, "invalid_params", err.to_string());
            }
        };
        let patch_format = params.patch_format.as_deref().unwrap_or("v4a");
        if patch_format != "v4a" {
            let payload = HostFsApplyPatchReceipt {
                status: "error".to_string(),
                files_changed: None,
                changed_paths: None,
                ops: None,
                summary_text: None,
                errors: None,
                error_code: Some("unsupported_patch_format".to_string()),
            };
            return build_receipt(
                intent,
                HOST_FS_APPLY_PATCH_FABRIC,
                ReceiptStatus::Error,
                &payload,
            );
        }

        let patch_text = match resolve_patch_text(self.store.as_ref(), &params) {
            Ok(text) => text,
            Err(err) => return fabric_fs_apply_patch_error(intent, "invalid_patch", err),
        };
        if patch_text.trim().is_empty() {
            return fabric_fs_apply_patch_error(
                intent,
                "invalid_patch_empty",
                "patch must not be empty".to_string(),
            );
        }

        let dry_run = params.dry_run.unwrap_or(false);
        let response = match self
            .shared
            .client
            .apply_patch(
                &SessionId(params.session_id),
                &FsApplyPatchRequest {
                    patch: patch_text,
                    patch_format: Some(patch_format.to_string()),
                    dry_run,
                },
            )
            .await
        {
            Ok(response) => response,
            Err(err) if fabric_error_code(&err) == "not_found" => {
                let payload = HostFsApplyPatchReceipt {
                    status: "not_found".to_string(),
                    files_changed: None,
                    changed_paths: None,
                    ops: None,
                    summary_text: None,
                    errors: Some(vec![err.to_string()]),
                    error_code: Some("path_not_found".to_string()),
                };
                return build_receipt(
                    intent,
                    HOST_FS_APPLY_PATCH_FABRIC,
                    ReceiptStatus::Ok,
                    &payload,
                );
            }
            Err(err) => {
                return fabric_fs_apply_patch_error(
                    intent,
                    fabric_error_code(&err),
                    err.to_string(),
                );
            }
        };

        let ops = HostPatchOpsSummary {
            add: response.ops.add,
            update: response.ops.update,
            delete: response.ops.delete,
            r#move: response.ops.move_count,
        };
        let summary = if response.applied {
            format!(
                "Applied patch: {} file(s) changed ({} add, {} update, {} delete, {} move)",
                response.files_changed, ops.add, ops.update, ops.delete, ops.r#move
            )
        } else {
            format!(
                "Patch dry-run: {} file(s) would change ({} add, {} update, {} delete, {} move)",
                response.files_changed, ops.add, ops.update, ops.delete, ops.r#move
            )
        };
        let payload = HostFsApplyPatchReceipt {
            status: "ok".to_string(),
            files_changed: Some(response.files_changed),
            changed_paths: Some(response.changed_paths),
            ops: Some(ops),
            summary_text: Some(summary),
            errors: None,
            error_code: None,
        };
        build_receipt(
            intent,
            HOST_FS_APPLY_PATCH_FABRIC,
            ReceiptStatus::Ok,
            &payload,
        )
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for FabricHostFsGrepAdapter<S> {
    fn kind(&self) -> &str {
        HOST_FS_GREP_FABRIC
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostFsGrepParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fabric_fs_grep_error(intent, "invalid_params", err.to_string()),
        };
        let mode = params.output_mode.as_deref().unwrap_or("auto");
        if !output_mode_valid(mode) {
            return fabric_fs_grep_error(
                intent,
                "invalid_output_mode",
                format!("unsupported output_mode '{mode}'"),
            );
        }

        let response = match self
            .shared
            .client
            .grep(
                &SessionId(params.session_id),
                &FsGrepRequest {
                    pattern: params.pattern,
                    path: params.path,
                    glob_filter: params.glob_filter,
                    max_results: params.max_results,
                    case_insensitive: params.case_insensitive.unwrap_or(false),
                },
            )
            .await
        {
            Ok(response) => response,
            Err(err) if fabric_error_code(&err) == "not_found" => {
                let payload = HostFsGrepReceipt {
                    status: "not_found".to_string(),
                    matches: None,
                    match_count: None,
                    truncated: None,
                    error_code: None,
                    summary_text: None,
                };
                return build_receipt(intent, HOST_FS_GREP_FABRIC, ReceiptStatus::Ok, &payload);
            }
            Err(err) => {
                return fabric_fs_grep_error(intent, fabric_error_code(&err), err.to_string());
            }
        };

        let text = format_grep_matches(&response.matches);
        let matches_payload =
            match materialize_text_output(self.store.as_ref(), mode, &text, self.output_cfg) {
                Ok(value) => value,
                Err(OutputMaterializeError::InlineRequiredTooLarge(_)) => {
                    return fabric_fs_grep_error(
                        intent,
                        "inline_required_too_large",
                        "grep output exceeds inline limit".to_string(),
                    );
                }
                Err(OutputMaterializeError::Store(err)) => {
                    return fabric_fs_grep_error(intent, "output_store_failed", err);
                }
            };
        let summary = if response.match_count == 0 {
            Some("No matches found".to_string())
        } else {
            Some(format!("{} match(es)", response.match_count))
        };
        let payload = HostFsGrepReceipt {
            status: "ok".to_string(),
            matches: matches_payload,
            match_count: Some(response.match_count),
            truncated: Some(response.truncated),
            error_code: None,
            summary_text: summary,
        };
        build_receipt(intent, HOST_FS_GREP_FABRIC, ReceiptStatus::Ok, &payload)
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for FabricHostFsGlobAdapter<S> {
    fn kind(&self) -> &str {
        HOST_FS_GLOB_FABRIC
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostFsGlobParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fabric_fs_glob_error(intent, "invalid_params", err.to_string()),
        };
        let mode = params.output_mode.as_deref().unwrap_or("auto");
        if !output_mode_valid(mode) {
            return fabric_fs_glob_error(
                intent,
                "invalid_output_mode",
                format!("unsupported output_mode '{mode}'"),
            );
        }

        let response = match self
            .shared
            .client
            .glob(
                &SessionId(params.session_id),
                &FsGlobRequest {
                    pattern: params.pattern,
                    path: params.path,
                    max_results: params.max_results,
                },
            )
            .await
        {
            Ok(response) => response,
            Err(err) if fabric_error_code(&err) == "not_found" => {
                let payload = HostFsGlobReceipt {
                    status: "not_found".to_string(),
                    paths: None,
                    count: None,
                    truncated: None,
                    error_code: None,
                    summary_text: None,
                };
                return build_receipt(intent, HOST_FS_GLOB_FABRIC, ReceiptStatus::Ok, &payload);
            }
            Err(err) => {
                return fabric_fs_glob_error(intent, fabric_error_code(&err), err.to_string());
            }
        };

        let text = response.paths.join("\n");
        let paths_payload =
            match materialize_text_output(self.store.as_ref(), mode, &text, self.output_cfg) {
                Ok(value) => value,
                Err(OutputMaterializeError::InlineRequiredTooLarge(_)) => {
                    return fabric_fs_glob_error(
                        intent,
                        "inline_required_too_large",
                        "glob output exceeds inline limit".to_string(),
                    );
                }
                Err(OutputMaterializeError::Store(err)) => {
                    return fabric_fs_glob_error(intent, "output_store_failed", err);
                }
            };
        let summary = if response.count == 0 {
            Some("No files matched".to_string())
        } else {
            Some(format!("{} path(s) matched", response.count))
        };
        let payload = HostFsGlobReceipt {
            status: "ok".to_string(),
            paths: paths_payload,
            count: Some(response.count),
            truncated: Some(response.truncated),
            error_code: None,
            summary_text: summary,
        };
        build_receipt(intent, HOST_FS_GLOB_FABRIC, ReceiptStatus::Ok, &payload)
    }
}

#[async_trait]
impl AsyncEffectAdapter for FabricHostFsStatAdapter {
    fn kind(&self) -> &str {
        HOST_FS_STAT_FABRIC
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostFsStatParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fabric_fs_stat_error(intent, "invalid_params", err.to_string()),
        };
        let response = match self
            .shared
            .client
            .stat(
                &SessionId(params.session_id),
                &FsPathQuery {
                    path: params.path,
                    offset_bytes: None,
                    max_bytes: None,
                },
            )
            .await
        {
            Ok(response) => response,
            Err(err) if fabric_error_code(&err) == "not_found" => {
                let payload = HostFsStatReceipt {
                    status: "not_found".to_string(),
                    exists: Some(false),
                    is_dir: None,
                    size_bytes: None,
                    mtime_ns: None,
                    error_code: None,
                };
                return build_receipt(intent, HOST_FS_STAT_FABRIC, ReceiptStatus::Ok, &payload);
            }
            Err(err) => {
                return fabric_fs_stat_error(intent, fabric_error_code(&err), err.to_string());
            }
        };
        let payload = HostFsStatReceipt {
            status: "ok".to_string(),
            exists: Some(true),
            is_dir: Some(response.kind == FsEntryKind::Directory),
            size_bytes: Some(response.size_bytes),
            mtime_ns: response.mtime_ns.map(u128_to_u64),
            error_code: None,
        };
        build_receipt(intent, HOST_FS_STAT_FABRIC, ReceiptStatus::Ok, &payload)
    }
}

#[async_trait]
impl AsyncEffectAdapter for FabricHostFsExistsAdapter {
    fn kind(&self) -> &str {
        HOST_FS_EXISTS_FABRIC
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostFsExistsParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fabric_fs_exists_error(intent, "invalid_params", err.to_string()),
        };
        let response = match self
            .shared
            .client
            .exists(
                &SessionId(params.session_id),
                &FsPathQuery {
                    path: params.path,
                    offset_bytes: None,
                    max_bytes: None,
                },
            )
            .await
        {
            Ok(response) => response,
            Err(err) if fabric_error_code(&err) == "not_found" => {
                let payload = HostFsExistsReceipt {
                    status: "ok".to_string(),
                    exists: Some(false),
                    error_code: None,
                };
                return build_receipt(intent, HOST_FS_EXISTS_FABRIC, ReceiptStatus::Ok, &payload);
            }
            Err(err) => {
                return fabric_fs_exists_error(intent, fabric_error_code(&err), err.to_string());
            }
        };
        let payload = HostFsExistsReceipt {
            status: "ok".to_string(),
            exists: Some(response.exists),
            error_code: None,
        };
        build_receipt(intent, HOST_FS_EXISTS_FABRIC, ReceiptStatus::Ok, &payload)
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for FabricHostFsListDirAdapter<S> {
    fn kind(&self) -> &str {
        HOST_FS_LIST_DIR_FABRIC
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HostFsListDirParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => return fabric_fs_list_dir_error(intent, "invalid_params", err.to_string()),
        };
        let mode = params.output_mode.as_deref().unwrap_or("auto");
        if !output_mode_valid(mode) {
            return fabric_fs_list_dir_error(
                intent,
                "invalid_output_mode",
                format!("unsupported output_mode '{mode}'"),
            );
        }
        let max_results = params.max_results.unwrap_or(100).min(usize::MAX as u64) as usize;

        let response = match self
            .shared
            .client
            .list_dir(
                &SessionId(params.session_id),
                &FsPathQuery {
                    path: params.path.unwrap_or_else(|| ".".to_string()),
                    offset_bytes: None,
                    max_bytes: None,
                },
            )
            .await
        {
            Ok(response) => response,
            Err(err) if fabric_error_code(&err) == "not_found" => {
                let payload = HostFsListDirReceipt {
                    status: "not_found".to_string(),
                    entries: None,
                    count: None,
                    truncated: None,
                    error_code: None,
                    summary_text: None,
                };
                return build_receipt(intent, HOST_FS_LIST_DIR_FABRIC, ReceiptStatus::Ok, &payload);
            }
            Err(err) => {
                return fabric_fs_list_dir_error(intent, fabric_error_code(&err), err.to_string());
            }
        };

        let total = response.entries.len();
        let mut text = String::new();
        for (idx, entry) in response.entries.iter().take(max_results).enumerate() {
            if idx > 0 {
                text.push('\n');
            }
            text.push_str(&entry.name);
            if entry.kind == FsEntryKind::Directory && !entry.name.ends_with('/') {
                text.push('/');
            }
        }
        let truncated = total > max_results;
        let entries_payload =
            match materialize_text_output(self.store.as_ref(), mode, &text, self.output_cfg) {
                Ok(value) => value,
                Err(OutputMaterializeError::InlineRequiredTooLarge(_)) => {
                    return fabric_fs_list_dir_error(
                        intent,
                        "inline_required_too_large",
                        "list_dir output exceeds inline limit".to_string(),
                    );
                }
                Err(OutputMaterializeError::Store(err)) => {
                    return fabric_fs_list_dir_error(intent, "output_store_failed", err);
                }
            };
        let payload = HostFsListDirReceipt {
            status: "ok".to_string(),
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
        build_receipt(intent, HOST_FS_LIST_DIR_FABRIC, ReceiptStatus::Ok, &payload)
    }
}

fn fabric_exec_progress_frame(
    intent: &EffectIntent,
    context: &AdapterStartContext,
    seq: u64,
    progress: ExecProgress,
) -> anyhow::Result<EffectStreamFrame> {
    let payload = HostExecProgressFrame {
        exec_id: progress.exec_id.map(|exec_id| exec_id.0),
        elapsed_ns: u128_to_u64(progress.elapsed.as_nanos()),
        stdout_delta: progress.stdout_delta,
        stderr_delta: progress.stderr_delta,
        stdout_bytes: progress.stdout_bytes,
        stderr_bytes: progress.stderr_bytes,
    };
    Ok(EffectStreamFrame {
        intent_hash: intent.intent_hash,
        adapter_id: HOST_EXEC_FABRIC.to_string(),
        origin_module_id: context.origin_module_id.clone(),
        origin_instance_key: context.origin_instance_key.clone(),
        effect_kind: context.effect_kind.clone(),
        emitted_at_seq: context.emitted_at_seq,
        seq,
        kind: "host.exec.progress".to_string(),
        payload_cbor: serde_cbor::to_vec(&payload)?,
        payload_ref: None,
        signature: vec![0; 64],
    })
}

fn controller_open_request(
    intent: &EffectIntent,
    config: &FabricAdapterConfig,
    params: &HostSessionOpenParams,
    target: &HostSandboxTarget,
) -> Result<ControllerSessionOpenRequest, String> {
    let image = if target.image.trim().is_empty() {
        config
            .default_image
            .clone()
            .ok_or_else(|| "sandbox image must not be empty".to_string())?
    } else {
        target.image.clone()
    };
    Ok(ControllerSessionOpenRequest {
        request_id: Some(request_id(intent)),
        target: FabricSessionTarget::Sandbox(FabricSandboxTarget {
            image,
            runtime_class: target
                .runtime_class
                .clone()
                .or_else(|| config.default_runtime_class.clone()),
            workdir: target.workdir.clone(),
            env: target.env.clone().unwrap_or_default(),
            network_mode: network_mode(
                target
                    .network_mode
                    .as_deref()
                    .or(config.default_network_mode.as_deref())
                    .unwrap_or("egress"),
            )?,
            mounts: target
                .mounts
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|mount| MountSpec {
                    host_path: mount.host_path,
                    guest_path: mount.guest_path,
                    read_only: matches!(mount.mode.as_str(), "ro" | "read_only" | "readonly"),
                })
                .collect(),
            resources: ResourceLimits {
                cpu_limit_millis: target.cpu_limit_millis,
                memory_limit_bytes: target.memory_limit_bytes,
            },
        }),
        ttl_ns: params.session_ttl_ns.map(u128::from),
        labels: params.labels.clone().unwrap_or_default(),
    })
}

fn request_id(intent: &EffectIntent) -> RequestId {
    RequestId(format!("aos:{}", hex::encode(intent.intent_hash)))
}

fn network_mode(raw: &str) -> Result<NetworkMode, String> {
    match raw {
        "egress" => Ok(NetworkMode::Egress),
        "disabled" | "none" => Ok(NetworkMode::Disabled),
        other => Err(format!("unsupported network_mode '{other}'")),
    }
}

fn fabric_signal(raw: &str) -> Result<FabricSessionSignal, String> {
    match raw {
        "close" | "term" | "terminate" => Ok(FabricSessionSignal::Close(CloseSignal {})),
        "quiesce" => Ok(FabricSessionSignal::Quiesce(QuiesceSignal {})),
        "resume" => Ok(FabricSessionSignal::Resume(ResumeSignal {})),
        other => Err(format!("unsupported signal '{other}'")),
    }
}

fn read_store_blob<S: Store>(store: &S, hash_ref: &str) -> Result<Vec<u8>, String> {
    let hash = Hash::from_hex_str(hash_ref).map_err(|err| err.to_string())?;
    store.get_blob(hash).map_err(|err| err.to_string())
}

fn fabric_error_code(error: &FabricClientError) -> &str {
    match error {
        FabricClientError::Server { code, .. } if code == "not_found" => "not_found",
        FabricClientError::Server { code, .. } if code == "no_healthy_host" => "no_healthy_host",
        FabricClientError::Server { code, .. } if code == "unsupported_target" => {
            "unsupported_target"
        }
        FabricClientError::Server { code, .. } if code == "unsupported_lifecycle" => {
            "unsupported_lifecycle"
        }
        FabricClientError::Server { .. } => "fabric_host_error",
        FabricClientError::Http(_) => "fabric_unreachable",
        FabricClientError::Json(_) | FabricClientError::InvalidPayload(_) => {
            "fabric_invalid_response"
        }
    }
}

fn controller_session_status(status: ControllerSessionStatus) -> &'static str {
    match status {
        ControllerSessionStatus::Creating => "creating",
        ControllerSessionStatus::Ready => "ready",
        ControllerSessionStatus::Quiesced => "quiesced",
        ControllerSessionStatus::Closing => "closing",
        ControllerSessionStatus::Closed => "closed",
        ControllerSessionStatus::Error => "error",
        ControllerSessionStatus::Lost => "lost",
        ControllerSessionStatus::HostUnreachable => "host_unreachable",
        ControllerSessionStatus::OrphanedHostSession => "orphaned_host_session",
    }
}

fn u128_to_u64(value: u128) -> u64 {
    value.min(u128::from(u64::MAX)) as u64
}

fn output_error_message(
    err: OutputMaterializeError,
    stream_name: &str,
    output_cfg: OutputConfig,
) -> String {
    match err {
        OutputMaterializeError::InlineRequiredTooLarge(len) => format!(
            "{stream_name} {len} bytes exceeds inline limit {}",
            output_cfg.inline_limit_bytes
        ),
        OutputMaterializeError::Store(err) => err,
    }
}

fn format_grep_matches(matches: &[FsGrepMatch]) -> String {
    let mut text = String::new();
    for (idx, item) in matches.iter().enumerate() {
        if idx > 0 {
            text.push('\n');
        }
        text.push_str(&item.path);
        text.push(':');
        text.push_str(&item.line_number.to_string());
        text.push(':');
        text.push_str(&item.line);
    }
    text
}

fn fabric_session_open_error(
    intent: &EffectIntent,
    started_at_ns: u64,
    code: &str,
    message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostSessionOpenReceipt {
        session_id: String::new(),
        status: "error".to_string(),
        started_at_ns,
        expires_at_ns: None,
        error_code: Some(code.to_string()),
        error_message: Some(message),
    };
    build_receipt(
        intent,
        HOST_SESSION_OPEN_FABRIC,
        ReceiptStatus::Error,
        &payload,
    )
}

fn fabric_exec_error(
    intent: &EffectIntent,
    started_at_ns: u64,
    exit_code: i32,
    code: &str,
    message: String,
    stdout: Option<HostOutput>,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostExecReceipt {
        exit_code,
        status: "error".to_string(),
        stdout,
        stderr: None,
        started_at_ns,
        ended_at_ns: now_wallclock_ns(),
        error_code: Some(code.to_string()),
        error_message: Some(message),
    };
    build_receipt(intent, HOST_EXEC_FABRIC, ReceiptStatus::Error, &payload)
}

fn fabric_signal_error(
    intent: &EffectIntent,
    code: &str,
    message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostSessionSignalReceipt {
        status: "error".to_string(),
        exit_code: None,
        ended_at_ns: None,
        error_code: Some(code.to_string()),
        error_message: Some(message),
    };
    build_receipt(
        intent,
        HOST_SESSION_SIGNAL_FABRIC,
        ReceiptStatus::Error,
        &payload,
    )
}

fn fabric_fs_read_error(
    intent: &EffectIntent,
    code: &str,
    message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsReadFileReceipt {
        status: if code == "not_found" {
            "not_found"
        } else {
            "error"
        }
        .to_string(),
        content: None,
        truncated: None,
        size_bytes: None,
        mtime_ns: None,
        error_code: if code == "not_found" {
            None
        } else {
            Some(code.to_string())
        },
        error_message: if code == "not_found" {
            None
        } else {
            Some(message)
        },
    };
    let receipt_status = if code == "not_found" {
        ReceiptStatus::Ok
    } else {
        ReceiptStatus::Error
    };
    build_receipt(intent, HOST_FS_READ_FILE_FABRIC, receipt_status, &payload)
}

fn fabric_fs_write_error(
    intent: &EffectIntent,
    code: &str,
    _message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsWriteFileReceipt {
        status: "error".to_string(),
        written_bytes: None,
        created: None,
        new_mtime_ns: None,
        error_code: Some(code.to_string()),
    };
    build_receipt(
        intent,
        HOST_FS_WRITE_FILE_FABRIC,
        ReceiptStatus::Error,
        &payload,
    )
}

fn fabric_fs_edit_error(
    intent: &EffectIntent,
    code: &str,
    _message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsEditFileReceipt {
        status: "error".to_string(),
        replacements: None,
        applied: Some(false),
        summary_text: None,
        error_code: Some(code.to_string()),
    };
    build_receipt(
        intent,
        HOST_FS_EDIT_FILE_FABRIC,
        ReceiptStatus::Error,
        &payload,
    )
}

fn fabric_fs_apply_patch_error(
    intent: &EffectIntent,
    code: &str,
    _message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsApplyPatchReceipt {
        status: "error".to_string(),
        files_changed: None,
        changed_paths: None,
        ops: None,
        summary_text: None,
        errors: None,
        error_code: Some(code.to_string()),
    };
    build_receipt(
        intent,
        HOST_FS_APPLY_PATCH_FABRIC,
        ReceiptStatus::Error,
        &payload,
    )
}

fn fabric_fs_grep_error(
    intent: &EffectIntent,
    code: &str,
    _message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsGrepReceipt {
        status: "error".to_string(),
        matches: None,
        match_count: None,
        truncated: None,
        error_code: Some(code.to_string()),
        summary_text: None,
    };
    build_receipt(intent, HOST_FS_GREP_FABRIC, ReceiptStatus::Error, &payload)
}

fn fabric_fs_glob_error(
    intent: &EffectIntent,
    code: &str,
    _message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsGlobReceipt {
        status: "error".to_string(),
        paths: None,
        count: None,
        truncated: None,
        error_code: Some(code.to_string()),
        summary_text: None,
    };
    build_receipt(intent, HOST_FS_GLOB_FABRIC, ReceiptStatus::Error, &payload)
}

fn fabric_fs_stat_error(
    intent: &EffectIntent,
    code: &str,
    _message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsStatReceipt {
        status: "error".to_string(),
        exists: None,
        is_dir: None,
        size_bytes: None,
        mtime_ns: None,
        error_code: Some(code.to_string()),
    };
    build_receipt(intent, HOST_FS_STAT_FABRIC, ReceiptStatus::Error, &payload)
}

fn fabric_fs_exists_error(
    intent: &EffectIntent,
    code: &str,
    _message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsExistsReceipt {
        status: "error".to_string(),
        exists: None,
        error_code: Some(code.to_string()),
    };
    build_receipt(
        intent,
        HOST_FS_EXISTS_FABRIC,
        ReceiptStatus::Error,
        &payload,
    )
}

fn fabric_fs_list_dir_error(
    intent: &EffectIntent,
    code: &str,
    _message: String,
) -> anyhow::Result<EffectReceipt> {
    let payload = HostFsListDirReceipt {
        status: "error".to_string(),
        entries: None,
        count: None,
        truncated: None,
        error_code: Some(code.to_string()),
        summary_text: None,
    };
    build_receipt(
        intent,
        HOST_FS_LIST_DIR_FABRIC,
        ReceiptStatus::Error,
        &payload,
    )
}
