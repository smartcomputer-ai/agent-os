use crate::contracts::{
    HostSessionStatus, RunId, SessionConfig, SessionId, SessionIngress, SessionIngressKind,
    SessionLifecycle, SessionLifecycleChanged, SessionState, default_tool_profile_for_provider,
    default_tool_profiles, default_tool_registry,
};
use crate::helpers::workflow::SessionReduceError;
use crate::{helpers::llm::LlmMappingError, tools::ToolEffectKind};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use aos_effects::builtins::{
    BlobGetParams, BlobPutParams, HostLocalTarget, HostSessionOpenParams, HostTarget,
    LlmGenerateParams, TextOrSecretRef,
};
use aos_wasm_sdk::{PendingEffect, ReduceError, Value, WorkflowCtx};

use super::llm::{LlmStepContext, materialize_llm_generate_params_with_prompt_refs};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEffectCommand {
    LlmGenerate {
        params: LlmGenerateParams,
        pending: PendingEffect,
    },
    ToolEffect {
        kind: ToolEffectKind,
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
            | Self::ToolEffect { pending, .. }
            | Self::BlobPut { pending, .. }
            | Self::BlobGet { pending, .. } => pending,
        }
    }

    pub fn emit(self, ctx: &mut WorkflowCtx<SessionState, Value>) {
        match self {
            Self::LlmGenerate { params, pending } => {
                ctx.effects().emit_raw_with_issuer_ref(
                    "llm.generate",
                    &params,
                    pending.cap_slot.as_deref(),
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
                    pending.cap_slot.as_deref(),
                    pending.issuer_ref.as_deref(),
                );
            }
            Self::BlobPut { params, pending } => {
                ctx.effects().emit_raw_with_issuer_ref(
                    "blob.put",
                    &params,
                    pending.cap_slot.as_deref(),
                    pending.issuer_ref.as_deref(),
                );
            }
            Self::BlobGet { params, pending } => {
                ctx.effects().emit_raw_with_issuer_ref(
                    "blob.get",
                    &params,
                    pending.cap_slot.as_deref(),
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
pub struct SessionReduceOutput {
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
    pub host_session_id: String,
    pub run_overrides: SessionConfig,
    pub allowed_tools: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHandoffPlan {
    pub ingresses: Vec<SessionIngress>,
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
    pub cap_slot: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedLlm {
    pub pending: PendingEffect,
    pub params: LlmGenerateParams,
}

pub fn request_llm(
    state: &mut SessionState,
    out: &mut SessionReduceOutput,
    mut request: RequestLlm,
) -> Result<RequestedLlm, SessionReduceError> {
    let run_config = state
        .active_run_config
        .clone()
        .ok_or(SessionReduceError::RunNotActive)?;
    if request.step.api_key.is_none() {
        request.step.api_key = provider_secret_ref(run_config.provider.as_str());
    }
    let params = materialize_llm_generate_params_with_prompt_refs(&run_config, request.step)
        .map_err(map_llm_mapping_error)?;
    let pending = begin_pending_effect(state, "llm.generate", &params, request.cap_slot, None);
    out.effects.push(SessionEffectCommand::LlmGenerate {
        params: params.clone(),
        pending: pending.clone(),
    });
    Ok(RequestedLlm { pending, params })
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
    let mut ingresses = Vec::new();
    let mut observed_at_ns = request.first_observed_at_ns;
    let mut run_overrides = request.run_overrides.clone();

    let tool_profile = run_overrides
        .default_tool_profile
        .clone()
        .unwrap_or_else(|| default_tool_profile_for_provider(run_overrides.provider.as_str()));
    run_overrides.default_tool_profile = Some(tool_profile.clone());

    if let Some(allowed_tools) = request.allowed_tools.clone() {
        let registry = default_tool_registry();
        let mut profiles = default_tool_profiles();
        profiles.insert(tool_profile.clone(), allowed_tools);
        ingresses.push(SessionIngress {
            session_id: request.session_id.clone(),
            observed_at_ns,
            ingress: SessionIngressKind::ToolRegistrySet {
                registry,
                profiles: Some(profiles),
                default_profile: Some(tool_profile.clone()),
            },
        });
        observed_at_ns = observed_at_ns.saturating_add(1);
    }

    ingresses.push(SessionIngress {
        session_id: request.session_id.clone(),
        observed_at_ns,
        ingress: SessionIngressKind::HostSessionUpdated {
            host_session_id: Some(request.host_session_id.clone()),
            host_session_status: Some(HostSessionStatus::Ready),
        },
    });
    observed_at_ns = observed_at_ns.saturating_add(1);

    ingresses.push(SessionIngress {
        session_id: request.session_id.clone(),
        observed_at_ns,
        ingress: SessionIngressKind::RunRequested {
            input_ref: request.input_ref.clone(),
            run_overrides: Some(run_overrides),
        },
    });
    observed_at_ns = observed_at_ns.saturating_add(1);

    SessionHandoffPlan {
        ingresses,
        next_observed_at_ns: observed_at_ns,
    }
}

pub fn emit_session_ingresses<S>(ctx: &mut WorkflowCtx<S, Value>, ingresses: &[SessionIngress]) {
    for ingress in ingresses {
        ctx.intent("aos.agent/SessionIngress@1")
            .payload(ingress)
            .send();
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
    effect_kind: &'static str,
    params: &T,
    cap_slot: Option<String>,
    issuer_ref: Option<String>,
) -> PendingEffect {
    let issuer_ref = issuer_ref.or_else(|| Some(synthesize_pending_issuer_ref(state, effect_kind)));
    match state.pending_effects.begin_with_issuer_ref(
        effect_kind,
        params,
        cap_slot.clone(),
        state.updated_at,
        issuer_ref.clone(),
    ) {
        Ok(pending) => pending,
        Err(_) => {
            let pending =
                PendingEffect::new(effect_kind, String::new(), cap_slot, state.updated_at)
                    .with_issuer_ref_opt(issuer_ref);
            state.pending_effects.insert(pending.clone());
            pending
        }
    }
}

fn synthesize_pending_issuer_ref(state: &SessionState, effect_kind: &str) -> String {
    let mut ordinal = state.pending_effects.len();
    loop {
        let candidate = format!("session:{effect_kind}:{}:{ordinal}", state.updated_at);
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

fn map_llm_mapping_error(err: LlmMappingError) -> SessionReduceError {
    match err {
        LlmMappingError::MissingProvider => SessionReduceError::MissingProvider,
        LlmMappingError::MissingModel => SessionReduceError::MissingModel,
        LlmMappingError::EmptyMessageRefs => SessionReduceError::EmptyMessageRefs,
        LlmMappingError::InvalidHashRef => SessionReduceError::InvalidHashRef,
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

pub fn map_reduce_error(err: SessionReduceError) -> ReduceError {
    match err {
        SessionReduceError::InvalidLifecycleTransition => {
            ReduceError::new("invalid lifecycle transition")
        }
        SessionReduceError::HostCommandRejected => ReduceError::new("host command rejected"),
        SessionReduceError::ToolBatchAlreadyActive => ReduceError::new("tool batch already active"),
        SessionReduceError::MissingProvider => ReduceError::new("run config provider missing"),
        SessionReduceError::MissingModel => ReduceError::new("run config model missing"),
        SessionReduceError::UnknownProvider => ReduceError::new("run config provider unknown"),
        SessionReduceError::UnknownModel => ReduceError::new("run config model unknown"),
        SessionReduceError::RunAlreadyActive => ReduceError::new("run already active"),
        SessionReduceError::RunNotActive => ReduceError::new("run not active"),
        SessionReduceError::EmptyMessageRefs => {
            ReduceError::new("llm message_refs must not be empty")
        }
        SessionReduceError::TooManyPendingEffects => ReduceError::new("too many pending effects"),
        SessionReduceError::InvalidHashRef => ReduceError::new("invalid hash ref"),
        SessionReduceError::ToolProfileUnknown => ReduceError::new("tool profile unknown"),
        SessionReduceError::UnknownToolOverride => ReduceError::new("unknown tool override"),
        SessionReduceError::InvalidToolRegistry => ReduceError::new("invalid tool registry"),
        SessionReduceError::AmbiguousPendingToolEffect => {
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
            params.target.as_local().workdir.as_deref(),
            Some("/tmp/project")
        );
    }

    #[test]
    fn handoff_plan_emits_registry_host_and_run_requested_in_order() {
        let plan = match spawn_or_handoff_session(SpawnOrHandoffSessionRequest::Handoff(
            SessionHandoffRequest {
                first_observed_at_ns: 10,
                session_id: SessionId("s-1".into()),
                input_ref:
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
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
                },
                allowed_tools: Some(vec!["shell".into(), "apply_patch".into()]),
            },
        )) {
            SpawnOrHandoffSessionPlan::Handoff(plan) => plan,
            other => panic!("unexpected plan: {other:?}"),
        };

        assert_eq!(plan.ingresses.len(), 3);
        assert_eq!(plan.next_observed_at_ns, 13);
        assert!(matches!(
            plan.ingresses[0].ingress,
            SessionIngressKind::ToolRegistrySet { .. }
        ));
        assert!(matches!(
            plan.ingresses[1].ingress,
            SessionIngressKind::HostSessionUpdated { .. }
        ));
        assert!(matches!(
            plan.ingresses[2].ingress,
            SessionIngressKind::RunRequested { .. }
        ));
    }
}
