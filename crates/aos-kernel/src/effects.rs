use std::collections::HashMap;
use std::sync::Arc;

use aos_air_exec::Value as ExprValue;
use aos_effects::builtins::{
    BlobGetReceipt, BlobPutReceipt, HttpRequestParams, LlmGenerateParams, LlmGenerateReceipt,
    TimerSetReceipt,
};
use aos_effects::{
    CapabilityGrant, EffectIntent, EffectKind, EffectReceipt, EffectSource, normalize_effect_params,
};
use aos_wasm_abi::ReducerEffect;
use serde::de::DeserializeOwned;
use serde_cbor::Value as CborValue;
use url::Url;

use crate::cap_enforcer::{CapCheckInput, CapEffectOrigin, CapEnforcerInvoker};
use crate::cap_ledger::{BudgetLedger, BudgetMap, CapReservation};
use crate::capability::{
    CapabilityResolver, CAP_ALLOW_ALL_ENFORCER, CAP_HTTP_ENFORCER, CAP_LLM_ENFORCER,
};
use crate::error::KernelError;
use crate::journal::{
    CapDecisionOutcome, CapDecisionRecord, CapDecisionStage, CapDenyReason,
};
use crate::policy::PolicyGate;
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

pub struct EffectManager {
    queue: EffectQueue,
    capability_gate: CapabilityResolver,
    policy_gate: Box<dyn PolicyGate>,
    effect_catalog: Arc<EffectCatalog>,
    schema_index: Arc<SchemaIndex>,
    cap_ledger: BudgetLedger,
    cap_reservations: HashMap<[u8; 32], CapReservation>,
    cap_decisions: Vec<CapDecisionRecord>,
    logical_now_ns: u64,
    enforcer_invoker: Option<Arc<dyn CapEnforcerInvoker>>,
    secret_catalog: Option<crate::secret::SecretCatalog>,
    secret_resolver: Option<Arc<dyn SecretResolver>>,
}

impl EffectManager {
    pub fn new(
        capability_gate: CapabilityResolver,
        policy_gate: Box<dyn PolicyGate>,
        effect_catalog: Arc<EffectCatalog>,
        schema_index: Arc<SchemaIndex>,
        cap_ledger: BudgetLedger,
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
            cap_ledger,
            cap_reservations: HashMap::new(),
            cap_decisions: Vec::new(),
            logical_now_ns: 0,
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

    fn enqueue_effect(
        &mut self,
        source: EffectSource,
        cap_name: &str,
        runtime_kind: EffectKind,
        params_cbor: Vec<u8>,
        idempotency_key: [u8; 32],
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

        let canonical_params = normalize_effect_params(
            &self.effect_catalog,
            &self.schema_index,
            &runtime_kind,
            &params_cbor,
        )
        .map_err(|err| KernelError::EffectManager(err.to_string()))?;
        let canonical_params = normalize_secret_variants(&canonical_params)
            .map_err(|err| KernelError::SecretResolution(err.to_string()))?;
        let resolved = self
            .capability_gate
            .resolve(cap_name, runtime_kind.as_str())?;
        let grant = resolved.grant;
        let enforcer_module = resolved.enforcer.module;
        let intent = EffectIntent::from_raw_params(
            runtime_kind.clone(),
            cap_name.to_string(),
            canonical_params.clone(),
            idempotency_key,
        )
        .map_err(|err| KernelError::EffectManager(err.to_string()))?;
        let cap_type = self
            .effect_catalog
            .cap_type(&runtime_kind)
            .ok_or_else(|| KernelError::UnsupportedEffectKind(runtime_kind.as_str().into()))?
            .as_str()
            .to_string();
        let reserve = match cap_constraints_and_reserve(
            enforcer_module.as_str(),
            &runtime_kind,
            &grant,
            &canonical_params,
            self.enforcer_invoker.as_ref(),
            &source,
            self.logical_now_ns,
        ) {
            Ok(reserve) => reserve,
            Err(reason) => {
                let message = reason.message.clone();
                self.record_cap_deny(
                    intent.intent_hash,
                    runtime_kind.as_str(),
                    cap_name,
                    &cap_type,
                    enforcer_module.as_str(),
                    grant.expiry_ns,
                    BudgetMap::new(),
                    reason,
                );
                return Err(KernelError::CapabilityDenied {
                    cap: cap_name.to_string(),
                    effect_kind: runtime_kind.as_str().to_string(),
                    reason: message,
                });
            }
        };
        if let Some(expiry_ns) = grant.expiry_ns {
            if self.logical_now_ns >= expiry_ns {
                self.record_cap_deny(
                    intent.intent_hash,
                    runtime_kind.as_str(),
                    cap_name,
                    &cap_type,
                    enforcer_module.as_str(),
                    grant.expiry_ns,
                    reserve.clone(),
                    CapDenyReason {
                        code: "expired".into(),
                        message: format!("grant expired at {expiry_ns}"),
                    },
                );
                return Err(KernelError::CapabilityDenied {
                    cap: cap_name.to_string(),
                    effect_kind: runtime_kind.as_str().to_string(),
                    reason: "cap grant expired".into(),
                });
            }
        }
        if let Err(err) = self.cap_ledger.check_reserve(&grant.name, &reserve) {
            self.record_cap_deny(
                intent.intent_hash,
                runtime_kind.as_str(),
                cap_name,
                &cap_type,
                enforcer_module.as_str(),
                grant.expiry_ns,
                reserve.clone(),
                CapDenyReason {
                    code: "budget_exceeded".into(),
                    message: err.to_string(),
                },
            );
            return Err(KernelError::CapabilityDenied {
                cap: cap_name.to_string(),
                effect_kind: runtime_kind.as_str().to_string(),
                reason: "cap budget exceeded".into(),
            });
        }
        if let Some(catalog) = &self.secret_catalog {
            crate::secret::enforce_secret_policy(&canonical_params, catalog, &source, cap_name)?;
        }
        match self.policy_gate.decide(&intent, &grant, &source)? {
            aos_effects::traits::PolicyDecision::Allow => {
                if let Err(err) = self.cap_ledger.apply_reserve(&grant.name, &reserve) {
                    self.record_cap_deny(
                        intent.intent_hash,
                        runtime_kind.as_str(),
                        cap_name,
                        &cap_type,
                        enforcer_module.as_str(),
                        grant.expiry_ns,
                        reserve.clone(),
                        CapDenyReason {
                            code: "budget_exceeded".into(),
                            message: err.to_string(),
                        },
                    );
                    return Err(KernelError::CapabilityDenied {
                        cap: cap_name.to_string(),
                        effect_kind: runtime_kind.as_str().to_string(),
                        reason: "cap budget exceeded".into(),
                    });
                }
                self.cap_reservations.insert(
                    intent.intent_hash,
                    CapReservation {
                        intent_hash: intent.intent_hash,
                        cap_name: cap_name.to_string(),
                        effect_kind: runtime_kind.as_str().to_string(),
                        cap_type: cap_type.clone(),
                        enforcer_module: enforcer_module.clone(),
                        reserve: reserve.clone(),
                        expiry_ns: grant.expiry_ns,
                    },
                );
                self.record_cap_allow(
                    intent.intent_hash,
                    runtime_kind.as_str(),
                    cap_name,
                    &cap_type,
                    enforcer_module.as_str(),
                    grant.expiry_ns,
                    reserve,
                );
                self.queue.push(intent.clone());
                Ok(intent)
            }
            aos_effects::traits::PolicyDecision::Deny => Err(KernelError::PolicyDenied {
                effect_kind: runtime_kind.as_str().to_string(),
                origin: format_effect_origin(&source),
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

    pub fn cap_ledger_snapshot(&self) -> BudgetLedger {
        self.cap_ledger.clone()
    }

    pub fn cap_reservations_snapshot(&self) -> Vec<CapReservation> {
        let mut reservations: Vec<CapReservation> =
            self.cap_reservations.values().cloned().collect();
        reservations.sort_by_key(|entry| entry.intent_hash);
        reservations
    }

    pub fn logical_now_ns(&self) -> u64 {
        self.logical_now_ns
    }

    pub fn restore_cap_state(
        &mut self,
        ledger: BudgetLedger,
        reservations: Vec<CapReservation>,
        logical_now_ns: u64,
    ) {
        self.cap_ledger = ledger;
        self.cap_reservations = reservations
            .into_iter()
            .map(|entry| (entry.intent_hash, entry))
            .collect();
        self.logical_now_ns = logical_now_ns;
    }

    pub fn update_cap_grants<I>(&mut self, grants: I)
    where
        I: IntoIterator<Item = (String, Option<aos_effects::CapabilityBudget>)>,
    {
        self.cap_ledger.update_from_grants(grants);
        self.cap_reservations.clear();
    }

    pub fn apply_cap_decision(&mut self, record: &CapDecisionRecord) -> Result<(), KernelError> {
        if record.logical_now_ns > self.logical_now_ns {
            self.logical_now_ns = record.logical_now_ns;
        }
        match record.stage {
            CapDecisionStage::Enqueue => {
                if record.decision == CapDecisionOutcome::Allow {
                    self.cap_ledger
                        .apply_reserve(&record.cap_name, &record.reserve)
                        .map_err(|err| KernelError::CapabilityDenied {
                            cap: record.cap_name.clone(),
                            effect_kind: record.effect_kind.clone(),
                            reason: err.to_string(),
                        })?;
                    self.cap_reservations.insert(
                        record.intent_hash,
                        CapReservation {
                            intent_hash: record.intent_hash,
                            cap_name: record.cap_name.clone(),
                            effect_kind: record.effect_kind.clone(),
                            cap_type: record.cap_type.clone(),
                            enforcer_module: record.enforcer_module.clone(),
                            reserve: record.reserve.clone(),
                            expiry_ns: record.expiry_ns,
                        },
                    );
                }
            }
            CapDecisionStage::Settle => {
                if record.decision == CapDecisionOutcome::Allow {
                    self.cap_ledger
                        .apply_settle(&record.cap_name, &record.reserve, &record.usage)
                        .map_err(|err| KernelError::CapabilityDenied {
                            cap: record.cap_name.clone(),
                            effect_kind: record.effect_kind.clone(),
                            reason: err.to_string(),
                        })?;
                    self.cap_reservations.remove(&record.intent_hash);
                }
            }
        }
        Ok(())
    }

    pub fn settle_receipt(&mut self, receipt: &EffectReceipt) -> Result<(), KernelError> {
        let Some(reservation) = self.cap_reservations.get(&receipt.intent_hash).cloned() else {
            return Ok(());
        };
        if reservation.effect_kind.as_str() == aos_effects::EffectKind::TIMER_SET {
            if let Ok(timer_receipt) = serde_cbor::from_slice::<TimerSetReceipt>(&receipt.payload_cbor)
            {
                self.logical_now_ns = self.logical_now_ns.max(timer_receipt.delivered_at_ns);
            }
        }
        let usage =
            usage_from_receipt(&reservation.effect_kind, receipt).map_err(|err| {
                KernelError::ReceiptDecode(err.to_string())
            })?;
        self.cap_ledger
            .apply_settle(&reservation.cap_name, &reservation.reserve, &usage)
            .map_err(|err| KernelError::CapabilityDenied {
                cap: reservation.cap_name.clone(),
                effect_kind: reservation.effect_kind.clone(),
                reason: err.to_string(),
            })?;
        self.cap_reservations.remove(&receipt.intent_hash);
        self.cap_decisions.push(CapDecisionRecord {
            intent_hash: receipt.intent_hash,
            stage: CapDecisionStage::Settle,
            effect_kind: reservation.effect_kind,
            cap_name: reservation.cap_name,
            cap_type: reservation.cap_type,
            enforcer_module: reservation.enforcer_module,
            decision: CapDecisionOutcome::Allow,
            deny: None,
            reserve: reservation.reserve,
            usage,
            expiry_ns: reservation.expiry_ns,
            logical_now_ns: self.logical_now_ns,
        });
        Ok(())
    }

    fn record_cap_deny(
        &mut self,
        intent_hash: [u8; 32],
        effect_kind: &str,
        cap_name: &str,
        cap_type: &str,
        enforcer_module: &str,
        expiry_ns: Option<u64>,
        reserve: BudgetMap,
        reason: CapDenyReason,
    ) {
        self.cap_decisions.push(CapDecisionRecord {
            intent_hash,
            stage: CapDecisionStage::Enqueue,
            effect_kind: effect_kind.to_string(),
            cap_name: cap_name.to_string(),
            cap_type: cap_type.to_string(),
            enforcer_module: enforcer_module.to_string(),
            decision: CapDecisionOutcome::Deny,
            deny: Some(reason),
            reserve,
            usage: BudgetMap::new(),
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
        enforcer_module: &str,
        expiry_ns: Option<u64>,
        reserve: BudgetMap,
    ) {
        self.cap_decisions.push(CapDecisionRecord {
            intent_hash,
            stage: CapDecisionStage::Enqueue,
            effect_kind: effect_kind.to_string(),
            cap_name: cap_name.to_string(),
            cap_type: cap_type.to_string(),
            enforcer_module: enforcer_module.to_string(),
            decision: CapDecisionOutcome::Allow,
            deny: None,
            reserve,
            usage: BudgetMap::new(),
            expiry_ns,
            logical_now_ns: self.logical_now_ns,
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

fn cap_constraints_and_reserve(
    enforcer_module: &str,
    kind: &EffectKind,
    grant: &CapabilityGrant,
    params_cbor: &[u8],
    enforcer_invoker: Option<&Arc<dyn CapEnforcerInvoker>>,
    origin: &EffectSource,
    logical_now_ns: u64,
) -> Result<BudgetMap, CapDenyReason> {
    if enforcer_module == CAP_ALLOW_ALL_ENFORCER {
        return Ok(BudgetMap::new());
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
) -> Result<BudgetMap, CapDenyReason> {
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
    let output = invoker
        .check(enforcer_module, input)
        .map_err(|err| CapDenyReason {
            code: "enforcer_error".into(),
            message: err.to_string(),
        })?;
    if output.constraints_ok {
        Ok(output.reserve_estimate)
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
) -> Result<BudgetMap, CapDenyReason> {
    if kind.as_str() != aos_effects::EffectKind::HTTP_REQUEST {
        return Err(CapDenyReason {
            code: "effect_kind_mismatch".into(),
            message: format!(
                "enforcer '{CAP_HTTP_ENFORCER}' cannot handle '{}'",
                kind.as_str()
            ),
        });
    }
    let cap_params: HttpCapParams = decode_cbor(&grant.params_cbor).map_err(|err| CapDenyReason {
        code: "cap_params_invalid".into(),
        message: err.to_string(),
    })?;
    let effect_params: HttpRequestParams = decode_cbor(params_cbor).map_err(|err| CapDenyReason {
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
    Ok(BudgetMap::new())
}

fn builtin_llm_enforcer(
    kind: &EffectKind,
    grant: &CapabilityGrant,
    params_cbor: &[u8],
) -> Result<BudgetMap, CapDenyReason> {
    if kind.as_str() != aos_effects::EffectKind::LLM_GENERATE {
        return Err(CapDenyReason {
            code: "effect_kind_mismatch".into(),
            message: format!(
                "enforcer '{CAP_LLM_ENFORCER}' cannot handle '{}'",
                kind.as_str()
            ),
        });
    }
    let cap_params: LlmCapParams = decode_cbor(&grant.params_cbor).map_err(|err| CapDenyReason {
        code: "cap_params_invalid".into(),
        message: err.to_string(),
    })?;
    let effect_params: LlmGenerateParamsView = decode_cbor(params_cbor).map_err(|err| {
        CapDenyReason {
            code: "effect_params_invalid".into(),
            message: err.to_string(),
        }
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
    let mut reserve = BudgetMap::new();
    if effect_params.max_tokens > 0 {
        reserve.insert("tokens".into(), effect_params.max_tokens);
    }
    Ok(reserve)
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

fn nat_from_expr(value: &ExprValue) -> Option<u64> {
    match value {
        ExprValue::Nat(n) => Some(*n),
        ExprValue::Int(n) if *n >= 0 => Some(*n as u64),
        _ => None,
    }
}

fn tokens_from_llm_expr(value: &ExprValue) -> Option<u64> {
    let ExprValue::Record(map) = value else {
        return None;
    };
    let token_usage = map.get("token_usage").and_then(|entry| match entry {
        ExprValue::Record(inner) => Some(inner),
        _ => None,
    });
    if let Some(token_usage) = token_usage {
        let prompt = token_usage.get("prompt").and_then(nat_from_expr)?;
        let completion = token_usage.get("completion").and_then(nat_from_expr)?;
        return Some(prompt + completion);
    }
    let prompt = map.get("tokens_prompt").and_then(nat_from_expr)?;
    let completion = map.get("tokens_completion").and_then(nat_from_expr)?;
    Some(prompt + completion)
}

fn usage_from_receipt(kind: &str, receipt: &EffectReceipt) -> Result<BudgetMap, serde_cbor::Error> {
    let mut usage = BudgetMap::new();
    match kind {
        aos_effects::EffectKind::LLM_GENERATE => {
            match serde_cbor::from_slice::<LlmGenerateReceipt>(&receipt.payload_cbor) {
                Ok(payload) => {
                    let tokens = payload.token_usage.prompt + payload.token_usage.completion;
                    if tokens > 0 {
                        usage.insert("tokens".into(), tokens);
                    }
                    if let Some(cost) = receipt.cost_cents.or(payload.cost_cents) {
                        usage.insert("cents".into(), cost);
                    }
                }
                Err(_) => {
                    let payload: ExprValue = serde_cbor::from_slice(&receipt.payload_cbor)?;
                    if let Some(tokens) = tokens_from_llm_expr(&payload) {
                        if tokens > 0 {
                            usage.insert("tokens".into(), tokens);
                        }
                    }
                    if let Some(cost) = receipt.cost_cents {
                        usage.insert("cents".into(), cost);
                    }
                }
            }
        }
        aos_effects::EffectKind::BLOB_PUT => {
            let payload: BlobPutReceipt = serde_cbor::from_slice(&receipt.payload_cbor)?;
            if payload.size > 0 {
                usage.insert("bytes".into(), payload.size);
            }
            if let Some(cost) = receipt.cost_cents {
                usage.insert("cents".into(), cost);
            }
        }
        aos_effects::EffectKind::BLOB_GET => {
            let payload: BlobGetReceipt = serde_cbor::from_slice(&receipt.payload_cbor)?;
            if payload.size > 0 {
                usage.insert("bytes".into(), payload.size);
            }
            if let Some(cost) = receipt.cost_cents {
                usage.insert("cents".into(), cost);
            }
        }
        _ => {
            if let Some(cost) = receipt.cost_cents {
                usage.insert("cents".into(), cost);
            }
        }
    }
    Ok(usage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::{CapType, builtins, catalog::EffectCatalog, plan_literals::SchemaIndex};
    use aos_effects::builtins::{HeaderMap, HttpRequestParams, LlmGenerateParams, LlmGenerateReceipt, TokenUsage};
    use aos_effects::{CapabilityGrant, EffectReceipt, EffectKind, ReceiptStatus};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn effect_manager_with_grants(
        grants: Vec<(CapabilityGrant, CapType)>,
    ) -> EffectManager {
        let resolver = CapabilityResolver::from_runtime_grants(grants);
        let cap_ledger = BudgetLedger::from_grants(resolver.grant_budgets());
        let effect_catalog = Arc::new(
            EffectCatalog::from_defs(builtins::builtin_effects().iter().map(|b| b.effect.clone())),
        );
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
            cap_ledger,
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
    fn llm_tokens_reserve_and_settle() {
        let grant = CapabilityGrant::builder(
            "cap_llm",
            "sys/llm.basic@1",
            &serde_json::json!({ "models": ["gpt-4"], "max_tokens": 50 }),
        )
        .budget(aos_effects::CapabilityBudget(
            [("tokens".to_string(), 100)].into_iter().collect(),
        ))
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
        let intent = mgr
            .enqueue_plan_effect(
                "plan",
                &EffectKind::llm_generate(),
                "cap_llm",
                params_cbor,
                [0u8; 32],
            )
            .expect("enqueue");

        let ledger = mgr.cap_ledger_snapshot();
        let entry = ledger
            .grants
            .get("cap_llm")
            .and_then(|dims| dims.get("tokens"))
            .expect("tokens entry");
        assert_eq!(entry.reserved, 50);

        let receipt_payload = LlmGenerateReceipt {
            output_ref: aos_air_types::HashRef::new(
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            )
            .expect("hash ref"),
            token_usage: TokenUsage {
                prompt: 10,
                completion: 20,
            },
            cost_cents: None,
            provider_id: "openai".into(),
        };
        let receipt = EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "adapter.llm".into(),
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt_payload).expect("encode receipt"),
            cost_cents: None,
            signature: vec![],
        };
        mgr.settle_receipt(&receipt).expect("settle");

        let ledger = mgr.cap_ledger_snapshot();
        let entry = ledger
            .grants
            .get("cap_llm")
            .and_then(|dims| dims.get("tokens"))
            .expect("tokens entry");
        assert_eq!(entry.reserved, 0);
        assert_eq!(entry.spent, 30);
    }

    #[test]
    fn expired_cap_is_denied() {
        let grant = CapabilityGrant::builder(
            "cap_http",
            "sys/http.out@1",
            &serde_json::json!({}),
        )
        .expiry_ns(10)
        .build()
        .expect("grant");
        let mut mgr = effect_manager_with_grants(vec![(grant, CapType::http_out())]);
        let cap_ledger = mgr.cap_ledger_snapshot();
        mgr.restore_cap_state(cap_ledger, vec![], 20);

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
