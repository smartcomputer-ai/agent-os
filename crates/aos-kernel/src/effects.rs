use std::sync::Arc;

use aos_air_types::EffectKind;
use aos_effects::{EffectIntent, EffectKind as RuntimeEffectKind, EffectSource};
use aos_wasm_abi::ReducerEffect;

use crate::capability::CapabilityResolver;
use crate::error::KernelError;
use crate::policy::PolicyGate;
use crate::secret::SecretResolver;

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
    secret_catalog: Option<crate::secret::SecretCatalog>,
    secret_resolver: Option<Arc<dyn SecretResolver>>,
}

impl EffectManager {
    pub fn new(
        capability_gate: CapabilityResolver,
        policy_gate: Box<dyn PolicyGate>,
        secret_catalog: Option<crate::secret::SecretCatalog>,
        secret_resolver: Option<Arc<dyn SecretResolver>>,
    ) -> Self {
        Self {
            queue: EffectQueue::default(),
            capability_gate,
            policy_gate,
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
        let runtime_kind = RuntimeEffectKind::new(effect.kind.clone());
        self.enqueue_effect(source, cap_name, runtime_kind, effect.params_cbor.clone())
    }

    pub fn drain(&mut self) -> Vec<EffectIntent> {
        self.queue.drain()
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
    ) -> Result<EffectIntent, KernelError> {
        let source = EffectSource::Plan {
            name: plan_name.to_string(),
        };
        let runtime_kind = RuntimeEffectKind::from_air(kind.clone());
        self.enqueue_effect(source, cap_name, runtime_kind, params_cbor)
    }

    fn enqueue_effect(
        &mut self,
        source: EffectSource,
        cap_name: &str,
        runtime_kind: RuntimeEffectKind,
        params_cbor: Vec<u8>,
    ) -> Result<EffectIntent, KernelError> {
        let original_params = params_cbor.clone();
        let grant = self
            .capability_gate
            .resolve(cap_name, runtime_kind.as_str())?;
        if let Some(catalog) = &self.secret_catalog {
            crate::secret::enforce_secret_policy(
                &original_params,
                catalog,
                &source,
                cap_name,
            )?;
        }
        let params_cbor = if let (Some(catalog), Some(resolver)) =
            (self.secret_catalog.as_ref(), self.secret_resolver.as_ref())
        {
            crate::secret::inject_secrets_in_params(&params_cbor, catalog, resolver.as_ref())
                .map_err(|err| KernelError::SecretResolution(err.to_string()))?
        } else {
            params_cbor
        };
        let intent = EffectIntent::from_raw_params(
            runtime_kind.clone(),
            cap_name.to_string(),
            params_cbor,
            [0u8; 32],
        )
        .map_err(|err| KernelError::EffectManager(err.to_string()))?;
        match self.policy_gate.decide(&intent, &grant, &source)? {
            aos_effects::traits::PolicyDecision::Allow => {
                self.queue.push(intent.clone());
                Ok(intent)
            }
            aos_effects::traits::PolicyDecision::Deny => Err(KernelError::PolicyDenied {
                effect_kind: runtime_kind.as_str().to_string(),
                origin: format_effect_origin(&source),
            }),
        }
    }

    pub fn restore_queue(&mut self, intents: Vec<EffectIntent>) {
        self.queue.set(intents);
    }

    pub fn secret_resolver(&self) -> Option<Arc<dyn SecretResolver>> {
        self.secret_resolver.clone()
    }
}

fn format_effect_origin(source: &EffectSource) -> String {
    match source {
        EffectSource::Reducer { name } => format!("reducer '{name}'"),
        EffectSource::Plan { name } => format!("plan '{name}'"),
    }
}
