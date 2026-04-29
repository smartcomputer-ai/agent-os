#![allow(improper_ctypes_definitions)]

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use aos_agent::{
    CauseRef, EffectReceiptRejected, ReasoningEffort, RunCause, RunCauseOrigin, RunId,
    SessionConfig, SessionId, SessionIngress, SessionIngressKind, SessionLifecycle,
    SessionLifecycleChanged, SessionNoop,
    helpers::{
        LocalSessionSpawnRequest, SessionHandoffRequest, SpawnOrHandoffSessionPlan,
        SpawnOrHandoffSessionRequest, emit_session_ingresses, spawn_or_handoff_session,
    },
    local_coding_agent_tool_registry,
};
use aos_effects::builtins::{BlobPutReceipt, HostSessionOpenReceipt};
use aos_wasm_sdk::{
    AirSchema, BlobPutParams, EffectReceiptEnvelope, ReduceError, Value, Workflow, WorkflowCtx,
    aos_workflow,
};
use serde::{Deserialize, Serialize};

aos_workflow!(Demiurge);

const DEFAULT_PROVIDER: &str = "openai-responses";
const DEFAULT_MODEL: &str = "gpt-5.3-codex";
const DEFAULT_MAX_TOKENS: u64 = 4096;
const EFFECT_HOST_SESSION_OPEN: &str = "sys/host.session.open@1";
const EFFECT_BLOB_PUT: &str = "sys/blob.put@1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "demiurge/TaskConfig@1")]
pub struct TaskConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub max_tokens: Option<u64>,
    pub tool_profile: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub tool_enable: Option<Vec<String>>,
    pub tool_disable: Option<Vec<String>>,
    pub tool_force: Option<Vec<String>>,
    pub session_ttl_ns: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "demiurge/TaskSubmitted@1")]
pub struct TaskSubmitted {
    pub task_id: SessionId,
    #[aos(air_type = "time")]
    pub observed_at_ns: u64,
    pub workdir: String,
    pub task: String,
    pub config: Option<TaskConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "demiurge/TaskStatus@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum TaskStatus {
    #[default]
    Idle,
    Bootstrapping,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "demiurge/TaskFailure@1")]
pub struct TaskFailure {
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, AirSchema)]
#[aos(schema = "demiurge/PendingStage@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum PendingStage {
    AwaitBlobPut,
    AwaitHostSessionOpen,
    AwaitRunCompletion,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "demiurge/TaskFinished@1")]
pub struct TaskFinished {
    pub task_id: SessionId,
    #[aos(air_type = "time")]
    pub observed_at_ns: u64,
    pub status: TaskStatus,
    pub failure: Option<TaskFailure>,
    pub run_id: Option<RunId>,
    #[aos(air_type = "hash")]
    pub output_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "demiurge/State@1")]
pub struct DemiurgeState {
    #[aos(schema_ref = SessionId)]
    pub task_id: SessionId,
    pub status: TaskStatus,
    pub workdir: Option<String>,
    pub task: Option<String>,
    pub config: Option<TaskConfig>,
    #[aos(air_type = "hash")]
    pub input_ref: Option<String>,
    #[aos(air_type = "hash")]
    pub output_ref: Option<String>,
    pub host_session_id: Option<String>,
    pub pending_stage: Option<PendingStage>,
    pub next_observed_at_ns: u64,
    pub finished: bool,
    pub failure: Option<TaskFailure>,
    #[aos(air_type = "time")]
    pub last_updated_at_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "demiurge/WorkflowEvent@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum DemiurgeWorkflowEvent {
    TaskSubmitted(TaskSubmitted),
    SessionLifecycleChanged(SessionLifecycleChanged),
    Receipt(EffectReceiptEnvelope),
    ReceiptRejected(EffectReceiptRejected),
    StreamFrame(aos_agent::EffectStreamFrameEnvelope),
    #[default]
    #[aos(schema_ref = SessionNoop)]
    Noop,
}

#[derive(Default)]
#[aos_wasm_sdk::air_workflow(
    name = "demiurge/Demiurge@1",
    module = "demiurge/Demiurge_wasm@1",
    state = DemiurgeState,
    event = DemiurgeWorkflowEvent,
    context = aos_wasm_sdk::WorkflowContext,
    key_schema = SessionId,
    effects = [
        aos_wasm_sdk::BlobPutParams,
        aos_wasm_sdk::HostSessionOpenParams,
    ]
)]
pub struct Demiurge;

impl Workflow for Demiurge {
    type State = DemiurgeState;
    type Event = DemiurgeWorkflowEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        ctx.state.last_updated_at_ns = event_observed_at_ns(&event);

        match event {
            DemiurgeWorkflowEvent::TaskSubmitted(task) => on_task_submitted(ctx, task),
            DemiurgeWorkflowEvent::SessionLifecycleChanged(changed) => {
                on_session_lifecycle_changed(ctx, changed)
            }
            DemiurgeWorkflowEvent::Receipt(receipt) => on_receipt(ctx, receipt),
            DemiurgeWorkflowEvent::ReceiptRejected(rejected) => on_receipt_rejected(ctx, rejected),
            DemiurgeWorkflowEvent::StreamFrame(_) | DemiurgeWorkflowEvent::Noop => Ok(()),
        }
    }
}

fn on_task_submitted(
    ctx: &mut WorkflowCtx<DemiurgeState, Value>,
    task: TaskSubmitted,
) -> Result<(), ReduceError> {
    if !matches!(ctx.state.status, TaskStatus::Idle) {
        return Ok(());
    }

    let config = task.config.clone().unwrap_or_default();

    if task.task_id.0.trim().is_empty() {
        return fail_task(
            ctx,
            task.observed_at_ns,
            "invalid_task_id",
            "task_id must be a non-empty UUID string",
            None,
        );
    }

    if task.workdir.trim().is_empty() || !is_absolute_path(task.workdir.as_str()) {
        return fail_task(
            ctx,
            task.observed_at_ns,
            "invalid_workdir",
            "workdir must be a non-empty absolute path",
            None,
        );
    }

    if let Some(err) = validate_allowed_tools(config.allowed_tools.as_deref()) {
        return fail_task(
            ctx,
            task.observed_at_ns,
            err.code.as_str(),
            err.detail.as_str(),
            None,
        );
    }

    ctx.state.task_id = task.task_id.clone();
    ctx.state.status = TaskStatus::Bootstrapping;
    ctx.state.workdir = Some(task.workdir.clone());
    ctx.state.task = Some(task.task.clone());
    ctx.state.config = Some(config);
    ctx.state.next_observed_at_ns = task.observed_at_ns.saturating_add(1);
    ctx.state.output_ref = None;

    let message_blob = UserMessageBlob {
        role: "user".into(),
        content: task.task,
    };
    let bytes = serde_json::to_vec(&message_blob)
        .map_err(|_| ReduceError::new("failed to encode task message JSON"))?;

    ctx.effects().emit_raw_with_issuer_ref(
        EFFECT_BLOB_PUT,
        &BlobPutParams {
            bytes,
            blob_ref: None,
            refs: None,
        },
        Some("blob"),
    );
    ctx.state.pending_stage = Some(PendingStage::AwaitBlobPut);
    Ok(())
}

fn on_receipt(
    ctx: &mut WorkflowCtx<DemiurgeState, Value>,
    receipt: EffectReceiptEnvelope,
) -> Result<(), ReduceError> {
    if ctx.state.finished {
        return Ok(());
    }

    let Some(stage) = ctx.state.pending_stage.clone() else {
        return Ok(());
    };

    match stage {
        PendingStage::AwaitBlobPut => {
            if receipt.effect != EFFECT_BLOB_PUT {
                return Ok(());
            }
            if receipt.status != "ok" {
                return fail_task(
                    ctx,
                    receipt.emitted_at_seq,
                    "blob_put_failed",
                    "blob.put receipt status was not ok",
                    None,
                );
            }

            let payload: BlobPutReceipt = serde_cbor::from_slice(&receipt.receipt_payload)
                .map_err(|_| ReduceError::new("blob.put receipt decode failed"))?;

            ctx.state.input_ref = Some(payload.blob_ref.as_str().into());
            let workdir = ctx
                .state
                .workdir
                .clone()
                .ok_or_else(|| ReduceError::new("missing workdir for host.session.open"))?;
            let cfg = ctx.state.config.clone().unwrap_or_default();

            let params = match spawn_or_handoff_session(SpawnOrHandoffSessionRequest::SpawnLocal(
                LocalSessionSpawnRequest {
                    workdir,
                    session_ttl_ns: cfg.session_ttl_ns,
                },
            )) {
                SpawnOrHandoffSessionPlan::OpenHostSession(params) => params,
                SpawnOrHandoffSessionPlan::Handoff(_) => unreachable!(),
            };
            ctx.effects()
                .emit_raw_with_issuer_ref(EFFECT_HOST_SESSION_OPEN, &params, Some("host"));
            ctx.state.pending_stage = Some(PendingStage::AwaitHostSessionOpen);
            Ok(())
        }
        PendingStage::AwaitHostSessionOpen => {
            if receipt.effect != EFFECT_HOST_SESSION_OPEN {
                return Ok(());
            }
            let payload: HostSessionOpenReceipt = serde_cbor::from_slice(&receipt.receipt_payload)
                .unwrap_or(HostSessionOpenReceipt {
                    session_id: String::new(),
                    status: String::from("error"),
                    started_at_ns: 0,
                    expires_at_ns: None,
                    error_code: Some(String::from("receipt_decode_error")),
                    error_message: Some(String::from("host.session.open receipt decode failed")),
                });

            if receipt.status != "ok" {
                let detail = format!(
                    "host.session.open envelope_status={} payload_status={} error_code={} error_message={}",
                    receipt.status,
                    payload.status,
                    payload.error_code.unwrap_or_default(),
                    payload.error_message.unwrap_or_default(),
                );
                return fail_task(
                    ctx,
                    receipt.emitted_at_seq,
                    "host_session_open_failed",
                    detail.as_str(),
                    None,
                );
            }

            if payload.status != "ready" || payload.session_id.trim().is_empty() {
                let detail = format!(
                    "host.session.open returned status={} session_id={} error_code={} error_message={}",
                    payload.status,
                    payload.session_id,
                    payload.error_code.unwrap_or_default(),
                    payload.error_message.unwrap_or_default(),
                );
                return fail_task(
                    ctx,
                    receipt.emitted_at_seq,
                    "host_session_not_ready",
                    detail.as_str(),
                    None,
                );
            }

            ctx.state.host_session_id = Some(payload.session_id.clone());
            ctx.state.pending_stage = None;
            ctx.state.status = TaskStatus::Running;
            emit_session_bootstrap(ctx, payload.session_id.as_str())?;
            Ok(())
        }
        PendingStage::AwaitRunCompletion => Ok(()),
    }
}

fn on_receipt_rejected(
    ctx: &mut WorkflowCtx<DemiurgeState, Value>,
    rejected: EffectReceiptRejected,
) -> Result<(), ReduceError> {
    if ctx.state.finished {
        return Ok(());
    }

    let Some(stage) = ctx.state.pending_stage.clone() else {
        return Ok(());
    };

    let expected = match stage {
        PendingStage::AwaitBlobPut => EFFECT_BLOB_PUT,
        PendingStage::AwaitHostSessionOpen => EFFECT_HOST_SESSION_OPEN,
        PendingStage::AwaitRunCompletion => return Ok(()),
    };
    if rejected.effect != expected {
        return Ok(());
    }

    let detail = format!(
        "{}: {}",
        rejected.error_code.as_str(),
        rejected.error_message.as_str()
    );
    fail_task(
        ctx,
        rejected.emitted_at_seq,
        "effect_rejected",
        detail.as_str(),
        None,
    )
}

fn emit_session_bootstrap(
    ctx: &mut WorkflowCtx<DemiurgeState, Value>,
    host_session_id: &str,
) -> Result<(), ReduceError> {
    let input_ref = ctx
        .state
        .input_ref
        .clone()
        .ok_or_else(|| ReduceError::new("missing input_ref for run request"))?;
    let cfg = ctx.state.config.clone().unwrap_or_default();

    let provider = cfg
        .provider
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_PROVIDER.into());
    let model = cfg
        .model
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.into());

    let run_overrides = SessionConfig {
        provider,
        model,
        reasoning_effort: cfg.reasoning_effort,
        max_tokens: Some(cfg.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS)),
        default_prompt_refs: None,
        default_tool_profile: cfg.tool_profile.clone(),
        default_tool_enable: cfg.tool_enable,
        default_tool_disable: cfg.tool_disable,
        default_tool_force: cfg.tool_force,
        default_host_session_open: None,
    };

    let first_observed_at_ns = next_observed_at_ns(&mut ctx.state);
    let run_cause = RunCause {
        kind: "demiurge/task_submitted".into(),
        origin: RunCauseOrigin::DomainEvent {
            schema: "demiurge/TaskSubmitted@1".into(),
            event_ref: None,
            key: Some(ctx.state.task_id.0.clone()),
        },
        input_refs: alloc::vec![input_ref.clone()],
        payload_schema: Some("demiurge/TaskSubmitted@1".into()),
        payload_ref: None,
        subject_refs: alloc::vec![CauseRef {
            kind: "demiurge/task".into(),
            id: ctx.state.task_id.0.clone(),
            ref_: None,
        }],
    };
    let plan = match spawn_or_handoff_session(SpawnOrHandoffSessionRequest::Handoff(
        SessionHandoffRequest {
            first_observed_at_ns,
            session_id: ctx.state.task_id.clone(),
            input_ref,
            run_cause: Some(run_cause),
            host_session_id: host_session_id.into(),
            run_overrides,
            allowed_tools: cfg.allowed_tools,
        },
    )) {
        SpawnOrHandoffSessionPlan::Handoff(plan) => plan,
        SpawnOrHandoffSessionPlan::OpenHostSession(_) => unreachable!(),
    };
    emit_session_ingresses(ctx, &plan.ingresses);
    ctx.state.next_observed_at_ns = plan.next_observed_at_ns;

    Ok(())
}

fn on_session_lifecycle_changed(
    ctx: &mut WorkflowCtx<DemiurgeState, Value>,
    changed: SessionLifecycleChanged,
) -> Result<(), ReduceError> {
    if ctx.state.finished {
        return Ok(());
    }
    if changed.session_id != ctx.state.task_id {
        return Ok(());
    }

    match changed.to {
        SessionLifecycle::Running => {
            ctx.state.status = TaskStatus::Running;
            Ok(())
        }
        SessionLifecycle::WaitingInput => request_run_completion(ctx, changed),
        SessionLifecycle::Completed => finish_task(
            ctx,
            changed.observed_at_ns,
            TaskStatus::Succeeded,
            None,
            changed.run_id,
            changed.output_ref,
        ),
        SessionLifecycle::Failed => finish_task(
            ctx,
            changed.observed_at_ns,
            TaskStatus::Failed,
            Some(TaskFailure {
                code: "session_failed".into(),
                detail: "aos.agent session entered Failed lifecycle".into(),
            }),
            changed.run_id,
            changed.output_ref,
        ),
        SessionLifecycle::Cancelled => finish_task(
            ctx,
            changed.observed_at_ns,
            TaskStatus::Cancelled,
            None,
            changed.run_id,
            changed.output_ref,
        ),
        SessionLifecycle::Interrupted => finish_task(
            ctx,
            changed.observed_at_ns,
            TaskStatus::Cancelled,
            None,
            changed.run_id,
            changed.output_ref,
        ),
        SessionLifecycle::Idle | SessionLifecycle::Paused | SessionLifecycle::Cancelling => Ok(()),
    }
}

fn request_run_completion(
    ctx: &mut WorkflowCtx<DemiurgeState, Value>,
    changed: SessionLifecycleChanged,
) -> Result<(), ReduceError> {
    if matches!(
        ctx.state.pending_stage,
        Some(PendingStage::AwaitRunCompletion)
    ) {
        return Ok(());
    }

    ctx.state.output_ref = changed.output_ref.clone();
    ctx.state.pending_stage = Some(PendingStage::AwaitRunCompletion);
    let observed_at_ns = next_observed_at_ns(&mut ctx.state);
    emit_session_ingresses(
        ctx,
        &[SessionIngress {
            session_id: changed.session_id,
            observed_at_ns,
            ingress: SessionIngressKind::RunCompleted,
        }],
    );
    Ok(())
}

fn finish_task(
    ctx: &mut WorkflowCtx<DemiurgeState, Value>,
    observed_at_ns: u64,
    status: TaskStatus,
    failure: Option<TaskFailure>,
    run_id: Option<RunId>,
    output_ref: Option<String>,
) -> Result<(), ReduceError> {
    if ctx.state.finished {
        return Ok(());
    }

    ctx.state.status = status.clone();
    ctx.state.failure = failure.clone();
    ctx.state.pending_stage = None;
    ctx.state.finished = true;
    ctx.state.output_ref = output_ref.clone();
    let task_id = ctx.state.task_id.clone();

    ctx.intent("demiurge/TaskFinished@1")
        .payload(&TaskFinished {
            task_id,
            observed_at_ns,
            status,
            failure,
            run_id,
            output_ref,
        })
        .send();

    Ok(())
}

fn fail_task(
    ctx: &mut WorkflowCtx<DemiurgeState, Value>,
    observed_at_ns: u64,
    code: &str,
    detail: &str,
    run_id: Option<RunId>,
) -> Result<(), ReduceError> {
    finish_task(
        ctx,
        observed_at_ns,
        TaskStatus::Failed,
        Some(TaskFailure {
            code: code.into(),
            detail: detail.into(),
        }),
        run_id,
        None,
    )
}

fn next_observed_at_ns(state: &mut DemiurgeState) -> u64 {
    if state.next_observed_at_ns == 0 {
        state.next_observed_at_ns = state.last_updated_at_ns.saturating_add(1);
    }
    let current = state.next_observed_at_ns;
    state.next_observed_at_ns = state.next_observed_at_ns.saturating_add(1);
    current
}

fn validate_allowed_tools(allowed_tools: Option<&[String]>) -> Option<TaskFailure> {
    let Some(allowed_tools) = allowed_tools else {
        return None;
    };
    if allowed_tools.is_empty() {
        return Some(TaskFailure {
            code: "invalid_allowed_tools".into(),
            detail: "allowed_tools must not be empty".into(),
        });
    }

    let registry = local_coding_agent_tool_registry();
    for tool_id in allowed_tools {
        if !registry.contains_key(tool_id) {
            return Some(TaskFailure {
                code: "unknown_allowed_tool".into(),
                detail: format!("unknown tool_id in allowed_tools: {tool_id}"),
            });
        }
    }
    None
}

fn is_absolute_path(value: &str) -> bool {
    value.starts_with('/') || value.starts_with("\\\\") || has_windows_drive_prefix(value)
}

fn has_windows_drive_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

fn event_observed_at_ns(event: &DemiurgeWorkflowEvent) -> u64 {
    match event {
        DemiurgeWorkflowEvent::TaskSubmitted(task) => task.observed_at_ns,
        DemiurgeWorkflowEvent::SessionLifecycleChanged(changed) => changed.observed_at_ns,
        DemiurgeWorkflowEvent::Receipt(receipt) => receipt.emitted_at_seq,
        DemiurgeWorkflowEvent::ReceiptRejected(rejected) => rejected.emitted_at_seq,
        DemiurgeWorkflowEvent::StreamFrame(frame) => frame.emitted_at_seq,
        DemiurgeWorkflowEvent::Noop => 0,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct UserMessageBlob {
    role: String,
    content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_path_validation_accepts_unix_and_windows() {
        assert!(is_absolute_path("/tmp/repo"));
        assert!(is_absolute_path("C:\\repo"));
        assert!(!is_absolute_path("repo"));
    }

    #[test]
    fn unknown_allowed_tool_is_rejected() {
        let err = validate_allowed_tools(Some(&["host.fs.nope".into()]))
            .expect("expected tool validation failure");
        assert_eq!(err.code, "unknown_allowed_tool");
    }
}
