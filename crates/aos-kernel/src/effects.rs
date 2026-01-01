use std::sync::Arc;

use aos_effects::builtins::{HttpRequestParams, LlmGenerateParams};
use aos_effects::{
    CapabilityGrant, EffectIntent, EffectKind, EffectSource, normalize_effect_params,
};
use aos_wasm_abi::{PureContext, ReducerEffect};
use serde::de::DeserializeOwned;
use serde_cbor::Value as CborValue;
use url::Url;

use crate::cap_enforcer::{CapCheckInput, CapEffectOrigin, CapEnforcerInvoker};
use crate::capability::{
    CAP_ALLOW_ALL_ENFORCER, CAP_HTTP_ENFORCER, CAP_LLM_ENFORCER, CapGrantResolution,
    CapabilityResolver,
};
use crate::error::KernelError;
use crate::journal::{
    CapDecisionOutcome, CapDecisionRecord, CapDecisionStage, CapDenyReason, PolicyDecisionOutcome,
    PolicyDecisionRecord,
};
use crate::policy::{PolicyDecisionDetail, PolicyGate};
use crate::secret::{SecretResolver, normalize_secret_variants};
use aos_air_types::catalog::EffectCatalog;
use aos_air_types::plan_literals::SchemaIndex;

#[derive(Default)]
pub struct EffectQueue {
    intents: Vec<EffectIntent>,
}

impl EffectQueue {
    pub fn push(&mut self, intent: EffectIntent) {
        self.intents.push(intent);
    }

    pub fn drain(&mut self) -> Vec<EffectIntent> {
        std::mem::take(&mut self.intents)
    }

    pub fn as_slice(&self) -> &[EffectIntent] {
        &self.intents
    }

    pub fn set(&mut self, intents: Vec<EffectIntent>) {
        self.intents = intents;
    }
}

#[derive(Clone, Debug)]
pub struct CapContext {
    pub logical_now_ns: u64,
    pub journal_height: u64,
    pub manifest_hash: String,
}

pub struct EffectManager {
    queue: EffectQueue,
    capability_gate: CapabilityResolver,
    policy_gate: Box<dyn PolicyGate>,
    effect_catalog: Arc<EffectCatalog>,
    schema_index: Arc<SchemaIndex>,
    param_preprocessor: Option<Arc<dyn EffectParamPreprocessor>>,
    cap_decisions: Vec<CapDecisionRecord>,
    policy_decisions: Vec<PolicyDecisionRecord>,
    logical_now_ns: u64,
    cap_context: Option<CapContext>,
    enforcer_invoker: Option<Arc<dyn CapEnforcerInvoker>>,
    secret_catalog: Option<crate::secret::SecretCatalog>,
    secret_resolver: Option<Arc<dyn SecretResolver>>,
}

pub trait EffectParamPreprocessor: Send + Sync {
    fn preprocess(
        &self,
        source: &EffectSource,
        kind: &EffectKind,
        params_cbor: Vec<u8>,
    ) -> Result<Vec<u8>, KernelError>;
}

impl EffectManager {
    pub fn new(
        capability_gate: CapabilityResolver,
        policy_gate: Box<dyn PolicyGate>,
        effect_catalog: Arc<EffectCatalog>,
        schema_index: Arc<SchemaIndex>,
        param_preprocessor: Option<Arc<dyn EffectParamPreprocessor>>,
        enforcer_invoker: Option<Arc<dyn CapEnforcerInvoker>>,
        secret_catalog: Option<crate::secret::SecretCatalog>,
        secret_resolver: Option<Arc<dyn SecretResolver>>,
    ) -> Self {
        Self {
            queue: EffectQueue::default(),
            capability_gate,
            policy_gate,
            effect_catalog,
            schema_index,
            param_preprocessor,
            cap_decisions: Vec::new(),
            policy_decisions: Vec::new(),
            logical_now_ns: 0,
            cap_context: None,
            enforcer_invoker,
            secret_catalog,
            secret_resolver,
        }
    }

    pub fn enqueue_reducer_effect(
        &mut self,
        reducer_name: &str,
        cap_name: &str,
        effect: &ReducerEffect,
    ) -> Result<EffectIntent, KernelError> {
        let source = EffectSource::Reducer {
            name: reducer_name.to_string(),
        };
        let runtime_kind = EffectKind::new(effect.kind.clone());
        let idempotency_key = normalize_idempotency_key(effect.idempotency_key.as_deref())?;
        self.enqueue_effect(
            source,
            cap_name,
            runtime_kind,
            effect.params_cbor.clone(),
            idempotency_key,
        )
    }

    pub fn unique_grant_for_effect_kind(
        &self,
        effect_kind: &str,
    ) -> Result<Option<CapGrantResolution>, KernelError> {
        self.capability_gate
            .unique_grant_for_effect_kind(effect_kind)
    }

    pub fn enqueue_reducer_effect_with_grant(
        &mut self,
        reducer_name: &str,
        grant: &CapGrantResolution,
        effect: &ReducerEffect,
    ) -> Result<EffectIntent, KernelError> {
        let source = EffectSource::Reducer {
            name: reducer_name.to_string(),
        };
        let runtime_kind = EffectKind::new(effect.kind.clone());
        let idempotency_key = normalize_idempotency_key(effect.idempotency_key.as_deref())?;
        self.enqueue_effect_with_grant(
            source,
            runtime_kind,
            effect.params_cbor.clone(),
            idempotency_key,
            grant,
        )
    }

    pub fn queued(&self) -> &[EffectIntent] {
        self.queue.as_slice()
    }

    pub fn enqueue_plan_effect(
        &mut self,
        plan_name: &str,
        kind: &EffectKind,
        cap_name: &str,
        params_cbor: Vec<u8>,
        idempotency_key: [u8; 32],
    ) -> Result<EffectIntent, KernelError> {
        let source = EffectSource::Plan {
            name: plan_name.to_string(),
        };
        let runtime_kind = kind.clone();
        self.enqueue_effect(source, cap_name, runtime_kind, params_cbor, idempotency_key)
    }

    pub fn enqueue_plan_effect_with_grant(
        &mut self,
        plan_name: &str,
        kind: &EffectKind,
        grant: &CapGrantResolution,
        params_cbor: Vec<u8>,
        idempotency_key: [u8; 32],
    ) -> Result<EffectIntent, KernelError> {
        let source = EffectSource::Plan {
            name: plan_name.to_string(),
        };
        let runtime_kind = kind.clone();
        self.enqueue_effect_with_grant(source, runtime_kind, params_cbor, idempotency_key, grant)
    }

    fn enqueue_effect(
        &mut self,
        source: EffectSource,
        cap_name: &str,
        runtime_kind: EffectKind,
        params_cbor: Vec<u8>,
        idempotency_key: [u8; 32],
    ) -> Result<EffectIntent, KernelError> {
        let resolved = self
            .capability_gate
            .resolve(cap_name, runtime_kind.as_str())?;
        self.enqueue_effect_with_grant(
            source,
            runtime_kind,
            params_cbor,
            idempotency_key,
            &resolved,
        )
    }

    fn enqueue_effect_with_grant(
        &mut self,
        source: EffectSource,
        runtime_kind: EffectKind,
        params_cbor: Vec<u8>,
        idempotency_key: [u8; 32],
        resolved: &CapGrantResolution,
    ) -> Result<EffectIntent, KernelError> {
        if let EffectSource::Reducer { .. } = &source {
            let scope = self
                .effect_catalog
                .origin_scope(&runtime_kind)
                .ok_or_else(|| KernelError::UnsupportedEffectKind(runtime_kind.as_str().into()))?;
            if !scope.allows_reducers() {
                return Err(KernelError::UnsupportedReducerReceipt(
                    runtime_kind.as_str().into(),
                ));
            }
        }

        let params_cbor = if let Some(preprocessor) = &self.param_preprocessor {
            preprocessor.preprocess(&source, &runtime_kind, params_cbor)?
        } else {
            params_cbor
        };

        let canonical_params = normalize_effect_params(
            &self.effect_catalog,
            &self.schema_index,
            &runtime_kind,
            &params_cbor,
        )
        .map_err(|err| KernelError::EffectManager(err.to_string()))?;
        let canonical_params = normalize_secret_variants(&canonical_params)
            .map_err(|err| KernelError::SecretResolution(err.to_string()))?;
        let grant = &resolved.grant;
        let enforcer_module = resolved.enforcer.module.as_str();
        let grant_hash = resolved.grant_hash;
        let intent = EffectIntent::from_raw_params(
            runtime_kind.clone(),
            grant.name.clone(),
            canonical_params.clone(),
            idempotency_key,
        )
        .map_err(|err| KernelError::EffectManager(err.to_string()))?;
        let expected_cap_type = self
            .effect_catalog
            .cap_type(&runtime_kind)
            .ok_or_else(|| KernelError::UnsupportedEffectKind(runtime_kind.as_str().into()))?;
        if &resolved.cap_type != expected_cap_type {
            return Err(KernelError::CapabilityTypeMismatch {
                grant: grant.name.clone(),
                effect_kind: runtime_kind.as_str().to_string(),
                expected: expected_cap_type.as_str().to_string(),
                found: resolved.cap_type.as_str().to_string(),
            });
        }
        let cap_type = expected_cap_type.as_str().to_string();
        if let Err(reason) = cap_constraints_only(
            enforcer_module,
            &runtime_kind,
            grant,
            &canonical_params,
            self.enforcer_invoker.as_ref(),
            &source,
            self.logical_now_ns,
            self.cap_context.as_ref(),
        ) {
            self.record_cap_deny(
                intent.intent_hash,
                runtime_kind.as_str(),
                grant.name.as_str(),
                &cap_type,
                grant_hash,
                enforcer_module,
                grant.expiry_ns,
                reason.clone(),
            );
            return Err(KernelError::CapabilityDenied {
                cap: grant.name.clone(),
                effect_kind: runtime_kind.as_str().to_string(),
                reason,
            });
        }
        if let Some(expiry_ns) = grant.expiry_ns {
            if self.logical_now_ns >= expiry_ns {
                let reason = CapDenyReason {
                    code: "expired".into(),
                    message: format!("grant expired at {expiry_ns}"),
                };
                self.record_cap_deny(
                    intent.intent_hash,
                    runtime_kind.as_str(),
                    grant.name.as_str(),
                    &cap_type,
                    grant_hash,
                    enforcer_module,
                    grant.expiry_ns,
                    reason.clone(),
                );
                return Err(KernelError::CapabilityDenied {
                    cap: grant.name.clone(),
                    effect_kind: runtime_kind.as_str().to_string(),
                    reason,
                });
            }
        }
        if let Some(catalog) = &self.secret_catalog {
            crate::secret::enforce_secret_policy(
                &canonical_params,
                catalog,
                &source,
                grant.name.as_str(),
            )?;
        }
        let policy_detail = self
            .policy_gate
            .decide(&intent, &grant, &source, &resolved.cap_type)?;
        let policy_decision = policy_detail.decision;
        self.record_policy_decision(intent.intent_hash, &policy_detail);
        match policy_decision {
            aos_effects::traits::PolicyDecision::Allow => {
                self.record_cap_allow(
                    intent.intent_hash,
                    runtime_kind.as_str(),
                    grant.name.as_str(),
                    &cap_type,
                    grant_hash,
                    enforcer_module,
                    grant.expiry_ns,
                );
                self.queue.push(intent.clone());
                Ok(intent)
            }
            aos_effects::traits::PolicyDecision::Deny => Err(KernelError::PolicyDenied {
                effect_kind: runtime_kind.as_str().to_string(),
                origin: format_effect_origin(&source),
                policy_name: policy_detail.policy_name,
                rule_index: policy_detail.rule_index,
            }),
        }
    }

    pub fn drain(&mut self) -> Vec<EffectIntent> {
        let mut intents = self.queue.drain();
        if let (Some(catalog), Some(resolver)) =
            (self.secret_catalog.as_ref(), self.secret_resolver.as_ref())
        {
            for intent in intents.iter_mut() {
                if let Ok(injected) = crate::secret::inject_secrets_in_params(
                    &intent.params_cbor,
                    catalog,
                    resolver.as_ref(),
                ) {
                    intent.params_cbor = injected;
                }
            }
        }
        intents
    }

    pub fn restore_queue(&mut self, intents: Vec<EffectIntent>) {
        self.queue.set(intents);
    }

    pub fn secret_resolver(&self) -> Option<Arc<dyn SecretResolver>> {
        self.secret_resolver.clone()
    }

    pub fn drain_cap_decisions(&mut self) -> Vec<CapDecisionRecord> {
        std::mem::take(&mut self.cap_decisions)
    }

    pub fn drain_policy_decisions(&mut self) -> Vec<PolicyDecisionRecord> {
        std::mem::take(&mut self.policy_decisions)
    }

    pub fn logical_now_ns(&self) -> u64 {
        self.logical_now_ns
    }

    pub fn update_logical_now_ns(&mut self, logical_now_ns: u64) {
        self.logical_now_ns = self.logical_now_ns.max(logical_now_ns);
    }

    pub fn set_cap_context(&mut self, context: CapContext) {
        self.cap_context = Some(context);
    }

    pub fn clear_cap_context(&mut self) {
        self.cap_context = None;
    }

    fn record_cap_deny(
        &mut self,
        intent_hash: [u8; 32],
        effect_kind: &str,
        cap_name: &str,
        cap_type: &str,
        grant_hash: [u8; 32],
        enforcer_module: &str,
        expiry_ns: Option<u64>,
        reason: CapDenyReason,
    ) {
        self.cap_decisions.push(CapDecisionRecord {
            intent_hash,
            stage: CapDecisionStage::Enqueue,
            effect_kind: effect_kind.to_string(),
            cap_name: cap_name.to_string(),
            cap_type: cap_type.to_string(),
            grant_hash,
            enforcer_module: enforcer_module.to_string(),
            decision: CapDecisionOutcome::Deny,
            deny: Some(reason),
            expiry_ns,
            logical_now_ns: self.logical_now_ns,
        });
    }

    fn record_cap_allow(
        &mut self,
        intent_hash: [u8; 32],
        effect_kind: &str,
        cap_name: &str,
        cap_type: &str,
        grant_hash: [u8; 32],
        enforcer_module: &str,
        expiry_ns: Option<u64>,
    ) {
        self.cap_decisions.push(CapDecisionRecord {
            intent_hash,
            stage: CapDecisionStage::Enqueue,
            effect_kind: effect_kind.to_string(),
            cap_name: cap_name.to_string(),
            cap_type: cap_type.to_string(),
            grant_hash,
            enforcer_module: enforcer_module.to_string(),
            decision: CapDecisionOutcome::Allow,
            deny: None,
            expiry_ns,
            logical_now_ns: self.logical_now_ns,
        });
    }

    fn record_policy_decision(
        &mut self,
        intent_hash: [u8; 32],
        detail: &PolicyDecisionDetail,
    ) {
        let outcome = match detail.decision {
            aos_effects::traits::PolicyDecision::Allow => PolicyDecisionOutcome::Allow,
            aos_effects::traits::PolicyDecision::Deny => PolicyDecisionOutcome::Deny,
        };
        self.policy_decisions.push(PolicyDecisionRecord {
            intent_hash,
            policy_name: detail.policy_name.clone(),
            rule_index: detail.rule_index,
            decision: outcome,
        });
    }
}

fn format_effect_origin(source: &EffectSource) -> String {
    match source {
        EffectSource::Reducer { name } => format!("reducer '{name}'"),
        EffectSource::Plan { name } => format!("plan '{name}'"),
    }
}

fn normalize_idempotency_key(value: Option<&[u8]>) -> Result<[u8; 32], KernelError> {
    match value {
        None => Ok([0u8; 32]),
        Some(bytes) => {
            let hash = aos_cbor::Hash::from_bytes(bytes).map_err(|err| {
                KernelError::IdempotencyKeyInvalid(format!("expected 32 bytes, got {}", err.0))
            })?;
            Ok(*hash.as_bytes())
        }
    }
}

#[derive(serde::Deserialize)]
struct HttpCapParams {
    hosts: Option<Vec<String>>,
    schemes: Option<Vec<String>>,
    methods: Option<Vec<String>>,
    ports: Option<Vec<u64>>,
    path_prefixes: Option<Vec<String>>,
}

#[derive(serde::Deserialize)]
struct LlmCapParams {
    providers: Option<Vec<String>>,
    models: Option<Vec<String>>,
    max_tokens: Option<u64>,
    tools_allow: Option<Vec<String>>,
}

#[derive(serde::Deserialize)]
struct LlmGenerateParamsView {
    provider: String,
    model: String,
    max_tokens: u64,
    #[serde(default)]
    tools: Option<Vec<String>>,
}

fn cap_constraints_only(
    enforcer_module: &str,
    kind: &EffectKind,
    grant: &CapabilityGrant,
    params_cbor: &[u8],
    enforcer_invoker: Option<&Arc<dyn CapEnforcerInvoker>>,
    origin: &EffectSource,
    logical_now_ns: u64,
    cap_context: Option<&CapContext>,
) -> Result<(), CapDenyReason> {
    if enforcer_module == CAP_ALLOW_ALL_ENFORCER {
        return Ok(());
    }
    if let Some(invoker) = enforcer_invoker {
        return invoke_enforcer(
            invoker,
            enforcer_module,
            kind,
            grant,
            params_cbor,
            origin,
            logical_now_ns,
            cap_context,
        );
    }
    match enforcer_module {
        CAP_HTTP_ENFORCER => builtin_http_enforcer(kind, grant, params_cbor),
        CAP_LLM_ENFORCER => builtin_llm_enforcer(kind, grant, params_cbor),
        _ => Err(CapDenyReason {
            code: "enforcer_not_found".into(),
            message: format!("enforcer module '{enforcer_module}' not available"),
        }),
    }
}

fn invoke_enforcer(
    invoker: &Arc<dyn CapEnforcerInvoker>,
    enforcer_module: &str,
    kind: &EffectKind,
    grant: &CapabilityGrant,
    params_cbor: &[u8],
    origin: &EffectSource,
    logical_now_ns: u64,
    cap_context: Option<&CapContext>,
) -> Result<(), CapDenyReason> {
    let origin = match origin {
        EffectSource::Reducer { name } => CapEffectOrigin {
            kind: "reducer".into(),
            name: name.clone(),
        },
        EffectSource::Plan { name } => CapEffectOrigin {
            kind: "plan".into(),
            name: name.clone(),
        },
    };
    let input = CapCheckInput {
        cap_def: grant.cap.clone(),
        grant_name: grant.name.clone(),
        cap_params: grant.params_cbor.clone(),
        effect_kind: kind.as_str().to_string(),
        effect_params: params_cbor.to_vec(),
        origin,
        logical_now_ns,
    };
    let context = cap_context.map(|ctx| PureContext {
        logical_now_ns: ctx.logical_now_ns,
        journal_height: ctx.journal_height,
        manifest_hash: ctx.manifest_hash.clone(),
        module: enforcer_module.to_string(),
    });
    let output = invoker
        .check(enforcer_module, input, context)
        .map_err(|err| CapDenyReason {
            code: "enforcer_error".into(),
            message: err.to_string(),
        })?;
    if output.constraints_ok {
        Ok(())
    } else {
        Err(output.deny.unwrap_or(CapDenyReason {
            code: "constraints_failed".into(),
            message: "cap enforcer denied request".into(),
        }))
    }
}

fn builtin_http_enforcer(
    kind: &EffectKind,
    grant: &CapabilityGrant,
    params_cbor: &[u8],
) -> Result<(), CapDenyReason> {
    if kind.as_str() != aos_effects::EffectKind::HTTP_REQUEST {
        return Err(CapDenyReason {
            code: "effect_kind_mismatch".into(),
            message: format!(
                "enforcer '{CAP_HTTP_ENFORCER}' cannot handle '{}'",
                kind.as_str()
            ),
        });
    }
    let cap_params: HttpCapParams =
        decode_cbor(&grant.params_cbor).map_err(|err| CapDenyReason {
            code: "cap_params_invalid".into(),
            message: err.to_string(),
        })?;
    let effect_params: HttpRequestParams =
        decode_cbor(params_cbor).map_err(|err| CapDenyReason {
            code: "effect_params_invalid".into(),
            message: err.to_string(),
        })?;
    let url = Url::parse(&effect_params.url).map_err(|err| CapDenyReason {
        code: "invalid_url".into(),
        message: err.to_string(),
    })?;
    let host = url.host_str().ok_or_else(|| CapDenyReason {
        code: "invalid_url".into(),
        message: "missing host".into(),
    })?;
    let scheme = url.scheme();
    let port = url.port_or_known_default();
    let path = url.path();

    if !allowlist_contains(&cap_params.hosts, host, |v| v.to_lowercase()) {
        return Err(CapDenyReason {
            code: "host_not_allowed".into(),
            message: format!("host '{host}' not allowed"),
        });
    }
    if !allowlist_contains(&cap_params.schemes, scheme, |v| v.to_lowercase()) {
        return Err(CapDenyReason {
            code: "scheme_not_allowed".into(),
            message: format!("scheme '{scheme}' not allowed"),
        });
    }
    if !allowlist_contains(&cap_params.methods, &effect_params.method, |v| {
        v.to_uppercase()
    }) {
        return Err(CapDenyReason {
            code: "method_not_allowed".into(),
            message: format!("method '{}' not allowed", effect_params.method),
        });
    }
    if let Some(ports) = &cap_params.ports {
        if !ports.is_empty() {
            let port = port.ok_or_else(|| CapDenyReason {
                code: "port_not_allowed".into(),
                message: "missing port".into(),
            })?;
            if !ports.iter().any(|p| *p == port as u64) {
                return Err(CapDenyReason {
                    code: "port_not_allowed".into(),
                    message: format!("port '{port}' not allowed"),
                });
            }
        }
    }
    if let Some(prefixes) = &cap_params.path_prefixes {
        if !prefixes.is_empty() && !prefixes.iter().any(|p| path.starts_with(p)) {
            return Err(CapDenyReason {
                code: "path_not_allowed".into(),
                message: format!("path '{path}' not allowed"),
            });
        }
    }
    Ok(())
}

fn builtin_llm_enforcer(
    kind: &EffectKind,
    grant: &CapabilityGrant,
    params_cbor: &[u8],
) -> Result<(), CapDenyReason> {
    if kind.as_str() != aos_effects::EffectKind::LLM_GENERATE {
        return Err(CapDenyReason {
            code: "effect_kind_mismatch".into(),
            message: format!(
                "enforcer '{CAP_LLM_ENFORCER}' cannot handle '{}'",
                kind.as_str()
            ),
        });
    }
    let cap_params: LlmCapParams =
        decode_cbor(&grant.params_cbor).map_err(|err| CapDenyReason {
            code: "cap_params_invalid".into(),
            message: err.to_string(),
        })?;
    let effect_params: LlmGenerateParamsView =
        decode_cbor(params_cbor).map_err(|err| CapDenyReason {
            code: "effect_params_invalid".into(),
            message: err.to_string(),
        })?;
    if !allowlist_contains(&cap_params.providers, &effect_params.provider, |v| {
        v.to_string()
    }) {
        return Err(CapDenyReason {
            code: "provider_not_allowed".into(),
            message: format!("provider '{}' not allowed", effect_params.provider),
        });
    }
    if !allowlist_contains(&cap_params.models, &effect_params.model, |v| v.to_string()) {
        return Err(CapDenyReason {
            code: "model_not_allowed".into(),
            message: format!("model '{}' not allowed", effect_params.model),
        });
    }
    if let Some(limit) = cap_params.max_tokens {
        if effect_params.max_tokens > limit {
            return Err(CapDenyReason {
                code: "max_tokens_exceeded".into(),
                message: format!(
                    "max_tokens {} exceeds cap {limit}",
                    effect_params.max_tokens
                ),
            });
        }
    }
    if let Some(allowed) = &cap_params.tools_allow {
        let tools = effect_params.tools.as_deref().unwrap_or(&[]);
        if !allowed.is_empty() && !tools.iter().all(|tool| allowed.iter().any(|t| t == tool)) {
            return Err(CapDenyReason {
                code: "tool_not_allowed".into(),
                message: "tool not allowed".into(),
            });
        }
    }
    Ok(())
}

fn decode_cbor<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, serde_cbor::Error> {
    const CBOR_SELF_DESCRIBE_TAG: u64 = 55799;
    let value: CborValue = serde_cbor::from_slice(bytes)?;
    let value = match value {
        CborValue::Tag(tag, inner) if tag == CBOR_SELF_DESCRIBE_TAG => *inner,
        other => other,
    };
    serde_cbor::value::from_value(value)
}

fn allowlist_contains(
    list: &Option<Vec<String>>,
    value: &str,
    normalize: impl Fn(&str) -> String,
) -> bool {
    let Some(list) = list else {
        return true;
    };
    if list.is_empty() {
        return true;
    }
    let value = normalize(value);
    list.iter().any(|entry| normalize(entry) == value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::{CapType, builtins, catalog::EffectCatalog, plan_literals::SchemaIndex};
    use aos_effects::builtins::{HeaderMap, HttpRequestParams, LlmGenerateParams};
    use aos_effects::{CapabilityGrant, EffectKind};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn effect_manager_with_grants(grants: Vec<(CapabilityGrant, CapType)>) -> EffectManager {
        let resolver = CapabilityResolver::from_runtime_grants(grants).expect("grant resolver");
        let effect_catalog = Arc::new(EffectCatalog::from_defs(
            builtins::builtin_effects().iter().map(|b| b.effect.clone()),
        ));
        let mut schemas = HashMap::new();
        for builtin in builtins::builtin_schemas() {
            schemas.insert(builtin.schema.name.clone(), builtin.schema.ty.clone());
        }
        let schema_index = Arc::new(SchemaIndex::new(schemas));
        EffectManager::new(
            resolver,
            Box::new(crate::policy::AllowAllPolicy),
            effect_catalog,
            schema_index,
            None,
            None,
            None,
            None,
        )
    }

    #[test]
    fn http_host_allowlist_is_enforced() {
        let grant = CapabilityGrant::builder(
            "cap_http",
            "sys/http.out@1",
            &serde_json::json!({ "hosts": ["example.com"] }),
        )
        .build()
        .expect("grant");
        let mut mgr = effect_manager_with_grants(vec![(grant, CapType::http_out())]);

        let params = HttpRequestParams {
            method: "GET".into(),
            url: "https://example.com/path".into(),
            headers: HeaderMap::new(),
            body_ref: None,
        };
        let params_cbor = serde_cbor::to_vec(&params).expect("encode params");
        let res = mgr.enqueue_plan_effect(
            "plan",
            &EffectKind::http_request(),
            "cap_http",
            params_cbor,
            [0u8; 32],
        );
        assert!(res.is_ok(), "enqueue failed: {:?}", res.err());

        let deny_params = HttpRequestParams {
            method: "GET".into(),
            url: "https://denied.example/path".into(),
            headers: HeaderMap::new(),
            body_ref: None,
        };
        let deny_cbor = serde_cbor::to_vec(&deny_params).expect("encode params");
        let err = mgr
            .enqueue_plan_effect(
                "plan",
                &EffectKind::http_request(),
                "cap_http",
                deny_cbor,
                [0u8; 32],
            )
            .expect_err("expected denial");
        assert!(
            matches!(err, KernelError::CapabilityDenied { .. }),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn llm_max_tokens_constraint_is_enforced() {
        let grant = CapabilityGrant::builder(
            "cap_llm",
            "sys/llm.basic@1",
            &serde_json::json!({ "models": ["gpt-4"], "max_tokens": 50 }),
        )
        .build()
        .expect("grant");
        let mut mgr = effect_manager_with_grants(vec![(grant, CapType::llm_basic())]);

        let params = LlmGenerateParams {
            provider: "openai".into(),
            model: "gpt-4".into(),
            temperature: "0.5".into(),
            max_tokens: 50,
            input_ref: aos_air_types::HashRef::new(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .expect("hash ref"),
            tools: None,
            api_key: None,
        };
        let params_cbor = serde_cbor::to_vec(&params).expect("encode params");
        let res = mgr.enqueue_plan_effect(
            "plan",
            &EffectKind::llm_generate(),
            "cap_llm",
            params_cbor,
            [0u8; 32],
        );
        assert!(res.is_ok(), "enqueue failed: {:?}", res.err());

        let over_limit = LlmGenerateParams {
            provider: "openai".into(),
            model: "gpt-4".into(),
            temperature: "0.5".into(),
            max_tokens: 55,
            input_ref: aos_air_types::HashRef::new(
                "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            )
            .expect("hash ref"),
            tools: None,
            api_key: None,
        };
        let over_cbor = serde_cbor::to_vec(&over_limit).expect("encode params");
        let err = mgr
            .enqueue_plan_effect(
                "plan",
                &EffectKind::llm_generate(),
                "cap_llm",
                over_cbor,
                [0u8; 32],
            )
            .expect_err("expected max_tokens denial");
        assert!(matches!(err, KernelError::CapabilityDenied { .. }));
    }

    #[test]
    fn expired_cap_is_denied() {
        let grant = CapabilityGrant::builder("cap_http", "sys/http.out@1", &serde_json::json!({}))
            .expiry_ns(10)
            .build()
            .expect("grant");
        let mut mgr = effect_manager_with_grants(vec![(grant, CapType::http_out())]);
        mgr.update_logical_now_ns(20);

        let params = HttpRequestParams {
            method: "GET".into(),
            url: "https://example.com/path".into(),
            headers: HeaderMap::new(),
            body_ref: None,
        };
        let params_cbor = serde_cbor::to_vec(&params).expect("encode params");
        let err = mgr
            .enqueue_plan_effect(
                "plan",
                &EffectKind::http_request(),
                "cap_http",
                params_cbor,
                [0u8; 32],
            )
            .expect_err("expected expiry denial");
        assert!(matches!(err, KernelError::CapabilityDenied { .. }));
    }
}
