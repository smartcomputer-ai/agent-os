use crate::contracts::{
    HostSessionStatus, RunCause, RunId, RunLifecycleChanged, RunState, SessionConfig, SessionId,
    SessionInput, SessionInputKind, SessionLifecycle, SessionLifecycleChanged, SessionState,
    SessionStatus, SessionStatusChanged, local_coding_agent_tool_profile_for_provider,
    local_coding_agent_tool_profiles, local_coding_agent_tool_registry,
};
use crate::helpers::workflow::SessionWorkflowError;
use crate::{helpers::llm::LlmMappingError, tools::ToolEffectOp};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use aos_effects::builtins::{
    BlobGetParams, BlobPutParams, HostLocalTarget, HostSessionOpenParams, HostTarget,
    LlmCompactParams, LlmGenerateParams, TextOrSecretRef,
};
use aos_wasm_sdk::{PendingEffect, ReduceError, Value, WorkflowCtx};

use super::llm::LlmCompactStepContext;
use super::llm::LlmStepContext;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEffectCommand {
    LlmGenerate {
        params: LlmGenerateParams,
        pending: PendingEffect,
    },
    LlmCompact {
        params: LlmCompactParams,
        pending: PendingEffect,
    },
    ToolEffect {
        kind: ToolEffectOp,
        params_json: String,
        pending: PendingEffect,
    },
    BlobPut {
        params: BlobPutParams,
        pending: PendingEffect,
    },
    BlobGet {
        params: BlobGetParams,
        pending: PendingEffect,
    },
}

impl SessionEffectCommand {
    pub fn pending(&self) -> &PendingEffect {
        match self {
            Self::LlmGenerate { pending, .. }
            | Self::LlmCompact { pending, .. }
            | Self::ToolEffect { pending, .. }
            | Self::BlobPut { pending, .. }
            | Self::BlobGet { pending, .. } => pending,
        }
    }

    pub fn emit(self, ctx: &mut WorkflowCtx<SessionState, Value>) {
        match self {
            Self::LlmGenerate { params, pending } => {
                ctx.effects().emit_raw_with_issuer_ref(
                    "sys/llm.generate@1",
                    &params,
                    pending.issuer_ref.as_deref(),
                );
            }
            Self::LlmCompact { params, pending } => {
                ctx.effects().emit_raw_with_issuer_ref(
                    "sys/llm.compact@1",
                    &params,
                    pending.issuer_ref.as_deref(),
                );
            }
            Self::ToolEffect {
                kind,
                params_json,
                pending,
            } => {
                let params: serde_json::Value =
                    serde_json::from_str(&params_json).unwrap_or(serde_json::Value::Null);
                ctx.effects().emit_raw_with_issuer_ref(
                    kind.as_str(),
                    &params,
                    pending.issuer_ref.as_deref(),
                );
            }
            Self::BlobPut { params, pending } => {
                ctx.effects().emit_raw_with_issuer_ref(
                    "sys/blob.put@1",
                    &params,
                    pending.issuer_ref.as_deref(),
                );
            }
            Self::BlobGet { params, pending } => {
                ctx.effects().emit_raw_with_issuer_ref(
                    "sys/blob.get@1",
                    &params,
                    pending.issuer_ref.as_deref(),
                );
            }
        }
    }

    pub fn params_hash(&self) -> &str {
        self.pending().params_hash.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDomainEventCommand {
    pub schema: &'static str,
    pub payload_json: String,
}

impl SessionDomainEventCommand {
    pub fn emit(self, ctx: &mut WorkflowCtx<SessionState, Value>) {
        let payload: serde_json::Value =
            serde_json::from_str(&self.payload_json).unwrap_or(serde_json::Value::Null);
        ctx.intent(self.schema).payload(&payload).send();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionWorkflowOutput {
    pub effects: alloc::vec::Vec<SessionEffectCommand>,
    pub domain_events: alloc::vec::Vec<SessionDomainEventCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSessionSpawnRequest {
    pub workdir: String,
    pub session_ttl_ns: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHandoffRequest {
    pub first_observed_at_ns: u64,
    pub session_id: SessionId,
    pub input_ref: String,
    pub run_cause: Option<RunCause>,
    pub host_session_id: String,
    pub run_overrides: SessionConfig,
    pub allowed_tools: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHandoffPlan {
    pub inputs: Vec<SessionInput>,
    pub next_observed_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnOrHandoffSessionRequest {
    SpawnLocal(LocalSessionSpawnRequest),
    Handoff(SessionHandoffRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnOrHandoffSessionPlan {
    OpenHostSession(HostSessionOpenParams),
    Handoff(SessionHandoffPlan),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestLlm {
    pub step: LlmStepContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestLlmCompact {
    pub step: LlmCompactStepContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedLlm {
    pub pending: PendingEffect,
    pub params: LlmGenerateParams,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedLlmCompact {
    pub pending: PendingEffect,
    pub params: LlmCompactParams,
}

pub fn request_llm(
    state: &mut SessionState,
    out: &mut SessionWorkflowOutput,
    mut request: RequestLlm,
) -> Result<RequestedLlm, SessionWorkflowError> {
    let run_config = state
        .active_run_config
        .clone()
        .ok_or(SessionWorkflowError::RunNotActive)?;
    if request.step.api_key.is_none() {
        request.step.api_key = provider_secret_ref(run_config.provider.as_str());
    }
    let params = crate::helpers::llm::materialize_llm_generate_params(&run_config, &request.step)
        .map_err(map_llm_mapping_error)?;
    let pending = begin_pending_effect(state, "sys/llm.generate@1", &params, None);
    out.effects.push(SessionEffectCommand::LlmGenerate {
        params: params.clone(),
        pending: pending.clone(),
    });
    Ok(RequestedLlm { pending, params })
}

pub fn request_llm_compact(
    state: &mut SessionState,
    out: &mut SessionWorkflowOutput,
    mut request: RequestLlmCompact,
) -> Result<RequestedLlmCompact, SessionWorkflowError> {
    let run_config = state
        .active_run_config
        .clone()
        .ok_or(SessionWorkflowError::RunNotActive)?;
    if request.step.api_key.is_none() {
        request.step.api_key = provider_secret_ref(run_config.provider.as_str());
    }
    let params = crate::helpers::llm::materialize_llm_compact_params(&run_config, &request.step)
        .map_err(map_llm_mapping_error)?;
    let pending = begin_pending_effect(state, "sys/llm.compact@1", &params, None);
    out.effects.push(SessionEffectCommand::LlmCompact {
        params: params.clone(),
        pending: pending.clone(),
    });
    Ok(RequestedLlmCompact { pending, params })
}

pub fn session_lifecycle_changed_payload(
    state: &SessionState,
    prev_lifecycle: SessionLifecycle,
    prev_run_id: Option<RunId>,
    observed_at_ns: u64,
) -> Option<SessionLifecycleChanged> {
    if prev_lifecycle == state.lifecycle {
        return None;
    }
    if state.session_id.0.is_empty() {
        return None;
    }

    Some(SessionLifecycleChanged {
        session_id: state.session_id.clone(),
        observed_at_ns,
        from: prev_lifecycle,
        to: state.lifecycle,
        run_id: state.active_run_id.clone().or(prev_run_id),
        output_ref: state.last_output_ref.clone(),
        in_flight_effects: state.in_flight_effects,
    })
}

pub fn emit_session_lifecycle_changed(
    ctx: &mut WorkflowCtx<SessionState, Value>,
    prev_lifecycle: SessionLifecycle,
    prev_run_id: Option<RunId>,
) {
    let observed_at_ns = ctx
        .logical_now_ns()
        .or_else(|| ctx.now_ns())
        .unwrap_or(ctx.state.updated_at);
    let Some(payload) =
        session_lifecycle_changed_payload(&ctx.state, prev_lifecycle, prev_run_id, observed_at_ns)
    else {
        return;
    };
    ctx.intent("aos.agent/SessionLifecycleChanged@1")
        .payload(&payload)
        .send();
}

pub fn session_status_changed_payload(
    state: &SessionState,
    prev_status: SessionStatus,
    observed_at_ns: u64,
) -> Option<SessionStatusChanged> {
    if prev_status == state.status || state.session_id.0.is_empty() {
        return None;
    }
    Some(SessionStatusChanged {
        session_id: state.session_id.clone(),
        observed_at_ns,
        from: prev_status,
        to: state.status,
    })
}

pub fn run_lifecycle_changed_payload(
    state: &SessionState,
    prev_run: Option<&RunState>,
    observed_at_ns: u64,
) -> Option<RunLifecycleChanged> {
    let current = state.current_run.as_ref();
    match (prev_run, current) {
        (Some(prev), Some(current)) if prev.lifecycle != current.lifecycle => {
            Some(RunLifecycleChanged {
                session_id: state.session_id.clone(),
                run_id: current.run_id.clone(),
                observed_at_ns,
                from: prev.lifecycle,
                to: current.lifecycle,
                cause: current.cause.clone(),
                output_ref: current
                    .outcome
                    .as_ref()
                    .and_then(|outcome| outcome.output_ref.clone()),
            })
        }
        (None, Some(current)) => Some(RunLifecycleChanged {
            session_id: state.session_id.clone(),
            run_id: current.run_id.clone(),
            observed_at_ns,
            from: crate::contracts::RunLifecycle::Queued,
            to: current.lifecycle,
            cause: current.cause.clone(),
            output_ref: current
                .outcome
                .as_ref()
                .and_then(|outcome| outcome.output_ref.clone()),
        }),
        (Some(prev), None) => {
            let record = state
                .run_history
                .iter()
                .rev()
                .find(|record| record.run_id == prev.run_id)?;
            Some(RunLifecycleChanged {
                session_id: state.session_id.clone(),
                run_id: record.run_id.clone(),
                observed_at_ns,
                from: prev.lifecycle,
                to: record.lifecycle,
                cause: record.cause.clone(),
                output_ref: record
                    .outcome
                    .as_ref()
                    .and_then(|outcome| outcome.output_ref.clone()),
            })
        }
        _ => None,
    }
}

pub fn emit_session_status_changed(
    ctx: &mut WorkflowCtx<SessionState, Value>,
    prev_status: SessionStatus,
) {
    let observed_at_ns = ctx
        .logical_now_ns()
        .or_else(|| ctx.now_ns())
        .unwrap_or(ctx.state.updated_at);
    let Some(payload) = session_status_changed_payload(&ctx.state, prev_status, observed_at_ns)
    else {
        return;
    };
    ctx.intent("aos.agent/SessionStatusChanged@1")
        .payload(&payload)
        .send();
}

pub fn emit_run_lifecycle_changed(
    ctx: &mut WorkflowCtx<SessionState, Value>,
    prev_run: Option<RunState>,
) {
    let observed_at_ns = ctx
        .logical_now_ns()
        .or_else(|| ctx.now_ns())
        .unwrap_or(ctx.state.updated_at);
    let Some(payload) =
        run_lifecycle_changed_payload(&ctx.state, prev_run.as_ref(), observed_at_ns)
    else {
        return;
    };
    ctx.intent("aos.agent/RunLifecycleChanged@1")
        .payload(&payload)
        .send();
}

pub fn local_session_open_params(request: &LocalSessionSpawnRequest) -> HostSessionOpenParams {
    HostSessionOpenParams {
        target: HostTarget::local(HostLocalTarget {
            mounts: None,
            workdir: Some(request.workdir.clone()),
            env: None,
            network_mode: "none".into(),
        }),
        session_ttl_ns: request.session_ttl_ns,
        labels: None,
    }
}

pub fn build_session_handoff_plan(request: &SessionHandoffRequest) -> SessionHandoffPlan {
    let mut inputs = Vec::new();
    let mut observed_at_ns = request.first_observed_at_ns;
    let mut run_overrides = request.run_overrides.clone();

    let tool_profile = run_overrides
        .default_tool_profile
        .clone()
        .unwrap_or_else(|| {
            local_coding_agent_tool_profile_for_provider(run_overrides.provider.as_str())
        });
    run_overrides.default_tool_profile = Some(tool_profile.clone());

    let registry = local_coding_agent_tool_registry();
    let mut profiles = local_coding_agent_tool_profiles();
    if let Some(allowed_tools) = request.allowed_tools.clone() {
        profiles.insert(tool_profile.clone(), allowed_tools);
    }
    inputs.push(SessionInput {
        session_id: request.session_id.clone(),
        observed_at_ns,
        input: SessionInputKind::ToolRegistrySet {
            registry,
            profiles: Some(profiles),
            default_profile: Some(tool_profile.clone()),
        },
    });
    observed_at_ns = observed_at_ns.saturating_add(1);

    inputs.push(SessionInput {
        session_id: request.session_id.clone(),
        observed_at_ns,
        input: SessionInputKind::HostSessionUpdated {
            host_session_id: Some(request.host_session_id.clone()),
            host_session_status: Some(HostSessionStatus::Ready),
        },
    });
    observed_at_ns = observed_at_ns.saturating_add(1);

    inputs.push(SessionInput {
        session_id: request.session_id.clone(),
        observed_at_ns,
        input: SessionInputKind::RunStartRequested {
            cause: request
                .run_cause
                .clone()
                .unwrap_or_else(|| RunCause::direct_input(request.input_ref.clone())),
            run_overrides: Some(run_overrides),
        },
    });
    observed_at_ns = observed_at_ns.saturating_add(1);

    SessionHandoffPlan {
        inputs,
        next_observed_at_ns: observed_at_ns,
    }
}

pub fn emit_session_inputs<S>(ctx: &mut WorkflowCtx<S, Value>, inputs: &[SessionInput]) {
    for input in inputs {
        ctx.intent("aos.agent/SessionInput@1").payload(input).send();
    }
}

pub fn spawn_or_handoff_session(
    request: SpawnOrHandoffSessionRequest,
) -> SpawnOrHandoffSessionPlan {
    match request {
        SpawnOrHandoffSessionRequest::SpawnLocal(request) => {
            SpawnOrHandoffSessionPlan::OpenHostSession(local_session_open_params(&request))
        }
        SpawnOrHandoffSessionRequest::Handoff(request) => {
            SpawnOrHandoffSessionPlan::Handoff(build_session_handoff_plan(&request))
        }
    }
}

pub fn begin_pending_effect<T: serde::Serialize>(
    state: &mut SessionState,
    effect: &'static str,
    params: &T,
    issuer_ref: Option<String>,
) -> PendingEffect {
    let issuer_ref = issuer_ref.or_else(|| Some(synthesize_pending_issuer_ref(state, effect)));
    match state.pending_effects.begin_with_issuer_ref(
        effect,
        params,
        state.updated_at,
        issuer_ref.clone(),
    ) {
        Ok(pending) => pending,
        Err(_) => {
            let pending = PendingEffect::new(effect, String::new(), state.updated_at)
                .with_issuer_ref_opt(issuer_ref);
            state.pending_effects.insert(pending.clone());
            pending
        }
    }
}

fn synthesize_pending_issuer_ref(state: &SessionState, effect: &str) -> String {
    let mut ordinal = state.pending_effects.len();
    loop {
        let candidate = format!("session:{effect}:{}:{ordinal}", state.updated_at);
        if state
            .pending_effects
            .values()
            .all(|pending| pending.issuer_ref.as_deref() != Some(candidate.as_str()))
        {
            return candidate;
        }
        ordinal += 1;
    }
}

fn map_llm_mapping_error(err: LlmMappingError) -> SessionWorkflowError {
    match err {
        LlmMappingError::MissingProvider => SessionWorkflowError::MissingProvider,
        LlmMappingError::MissingModel => SessionWorkflowError::MissingModel,
        LlmMappingError::EmptyWindowItems => SessionWorkflowError::EmptyMessageRefs,
        LlmMappingError::InvalidHashRef => SessionWorkflowError::InvalidHashRef,
    }
}

fn provider_secret_ref(provider: &str) -> Option<TextOrSecretRef> {
    let normalized = provider.trim().to_ascii_lowercase();
    if normalized.contains("anthropic") {
        return Some(TextOrSecretRef::secret("llm/anthropic_api", 1));
    }
    if normalized.contains("openai") {
        return Some(TextOrSecretRef::secret("llm/openai_api", 1));
    }
    None
}

pub fn map_workflow_error(err: SessionWorkflowError) -> ReduceError {
    match err {
        SessionWorkflowError::InvalidLifecycleTransition => {
            ReduceError::new("invalid lifecycle transition")
        }
        SessionWorkflowError::HostCommandRejected => ReduceError::new("host command rejected"),
        SessionWorkflowError::ToolBatchAlreadyActive => {
            ReduceError::new("tool batch already active")
        }
        SessionWorkflowError::MissingProvider => ReduceError::new("run config provider missing"),
        SessionWorkflowError::MissingModel => ReduceError::new("run config model missing"),
        SessionWorkflowError::UnknownProvider => ReduceError::new("run config provider unknown"),
        SessionWorkflowError::UnknownModel => ReduceError::new("run config model unknown"),
        SessionWorkflowError::RunAlreadyActive => ReduceError::new("run already active"),
        SessionWorkflowError::RunNotActive => ReduceError::new("run not active"),
        SessionWorkflowError::EmptyMessageRefs => {
            ReduceError::new("llm window_items must not be empty")
        }
        SessionWorkflowError::UnrenderableActiveWindowItem => {
            ReduceError::new("active window item cannot be rendered for provider")
        }
        SessionWorkflowError::TooManyPendingEffects => ReduceError::new("too many pending effects"),
        SessionWorkflowError::InvalidHashRef => ReduceError::new("invalid hash ref"),
        SessionWorkflowError::ToolProfileUnknown => ReduceError::new("tool profile unknown"),
        SessionWorkflowError::UnknownToolOverride => ReduceError::new("unknown tool override"),
        SessionWorkflowError::InvalidToolRegistry => ReduceError::new("invalid tool registry"),
        SessionWorkflowError::AmbiguousPendingToolEffect => {
            ReduceError::new("ambiguous pending tool effect")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReasoningEffort, SessionState};
    use alloc::vec;

    #[test]
    fn lifecycle_payload_emits_on_transition() {
        let payload = session_lifecycle_changed_payload(
            &SessionState {
                session_id: SessionId("11111111-1111-1111-1111-111111111111".into()),
                lifecycle: SessionLifecycle::Running,
                active_run_id: Some(RunId {
                    session_id: SessionId("11111111-1111-1111-1111-111111111111".into()),
                    run_seq: 1,
                }),
                in_flight_effects: 3,
                ..SessionState::default()
            },
            SessionLifecycle::Idle,
            None,
            42,
        )
        .expect("lifecycle event payload");
        assert_eq!(payload.observed_at_ns, 42);
        assert_eq!(payload.from, SessionLifecycle::Idle);
        assert_eq!(payload.to, SessionLifecycle::Running);
        assert_eq!(payload.in_flight_effects, 3);
        assert!(payload.run_id.is_some());
    }

    #[test]
    fn lifecycle_payload_uses_previous_run_id_when_terminal_clears_active() {
        let session_id = SessionId("11111111-1111-1111-1111-111111111111".into());
        let run_id = RunId {
            session_id: session_id.clone(),
            run_seq: 7,
        };
        let payload = session_lifecycle_changed_payload(
            &SessionState {
                session_id: session_id.clone(),
                lifecycle: SessionLifecycle::Failed,
                active_run_id: None,
                ..SessionState::default()
            },
            SessionLifecycle::Running,
            Some(run_id.clone()),
            88,
        )
        .expect("lifecycle event payload");
        assert_eq!(payload.to, SessionLifecycle::Failed);
        assert_eq!(payload.run_id, Some(run_id));
    }

    #[test]
    fn lifecycle_payload_skips_when_unchanged() {
        let payload = session_lifecycle_changed_payload(
            &SessionState {
                session_id: SessionId("11111111-1111-1111-1111-111111111111".into()),
                lifecycle: SessionLifecycle::Running,
                ..SessionState::default()
            },
            SessionLifecycle::Running,
            None,
            0,
        );
        assert!(payload.is_none());
    }

    #[test]
    fn spawn_local_builds_host_session_open_params() {
        let params = match spawn_or_handoff_session(SpawnOrHandoffSessionRequest::SpawnLocal(
            LocalSessionSpawnRequest {
                workdir: "/tmp/project".into(),
                session_ttl_ns: Some(99),
            },
        )) {
            SpawnOrHandoffSessionPlan::OpenHostSession(params) => params,
            other => panic!("unexpected plan: {other:?}"),
        };
        assert_eq!(params.session_ttl_ns, Some(99));
        assert_eq!(
            params
                .target
                .as_local()
                .and_then(|target| target.workdir.as_deref()),
            Some("/tmp/project")
        );
    }

    #[test]
    fn handoff_plan_emits_registry_host_and_run_start_requested_in_order() {
        let plan = match spawn_or_handoff_session(SpawnOrHandoffSessionRequest::Handoff(
            SessionHandoffRequest {
                first_observed_at_ns: 10,
                session_id: SessionId("s-1".into()),
                input_ref:
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                run_cause: None,
                host_session_id: "hs_1".into(),
                run_overrides: SessionConfig {
                    provider: "openai".into(),
                    model: "gpt-5.2".into(),
                    reasoning_effort: Some(ReasoningEffort::Medium),
                    max_tokens: Some(512),
                    default_prompt_refs: None,
                    default_tool_profile: None,
                    default_tool_enable: Some(vec!["shell".into()]),
                    default_tool_disable: None,
                    default_tool_force: None,
                    default_host_session_open: None,
                },
                allowed_tools: Some(vec!["shell".into(), "apply_patch".into()]),
            },
        )) {
            SpawnOrHandoffSessionPlan::Handoff(plan) => plan,
            other => panic!("unexpected plan: {other:?}"),
        };

        assert_eq!(plan.inputs.len(), 3);
        assert_eq!(plan.next_observed_at_ns, 13);
        assert!(matches!(
            plan.inputs[0].input,
            SessionInputKind::ToolRegistrySet { .. }
        ));
        assert!(matches!(
            plan.inputs[1].input,
            SessionInputKind::HostSessionUpdated { .. }
        ));
        assert!(matches!(
            plan.inputs[2].input,
            SessionInputKind::RunStartRequested { .. }
        ));
    }

    #[test]
    fn handoff_plan_emits_default_registry_without_allowed_tool_override() {
        let plan = match spawn_or_handoff_session(SpawnOrHandoffSessionRequest::Handoff(
            SessionHandoffRequest {
                first_observed_at_ns: 10,
                session_id: SessionId("s-1".into()),
                input_ref:
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                run_cause: None,
                host_session_id: "hs_1".into(),
                run_overrides: SessionConfig {
                    provider: "openai-responses".into(),
                    model: "gpt-5.3-codex".into(),
                    reasoning_effort: None,
                    max_tokens: Some(512),
                    default_prompt_refs: None,
                    default_tool_profile: None,
                    default_tool_enable: None,
                    default_tool_disable: None,
                    default_tool_force: None,
                    default_host_session_open: None,
                },
                allowed_tools: None,
            },
        )) {
            SpawnOrHandoffSessionPlan::Handoff(plan) => plan,
            other => panic!("unexpected plan: {other:?}"),
        };

        assert_eq!(plan.inputs.len(), 3);
        let SessionInputKind::ToolRegistrySet {
            registry,
            profiles,
            default_profile,
        } = &plan.inputs[0].input
        else {
            panic!("expected ToolRegistrySet first");
        };
        assert!(registry.contains_key("host.exec"));
        assert_eq!(default_profile.as_deref(), Some("openai"));
        assert!(
            profiles
                .as_ref()
                .and_then(|profiles| profiles.get("openai"))
                .is_some_and(|tools| tools.iter().any(|tool| tool == "host.exec"))
        );
    }
}
