use aos_air_types::EffectKind;
use aos_effects::{EffectIntent, EffectKind as RuntimeEffectKind, EffectSource};
use aos_wasm_abi::ReducerEffect;

use crate::capability::{AllowAllPolicy, CapabilityResolver, PolicyGate};
use crate::error::KernelError;

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
}

pub struct EffectManager {
    queue: EffectQueue,
    capability_gate: CapabilityResolver,
    policy_gate: AllowAllPolicy,
}

impl EffectManager {
    pub fn new(capability_gate: CapabilityResolver, policy_gate: AllowAllPolicy) -> Self {
        Self {
            queue: EffectQueue::default(),
            capability_gate,
            policy_gate,
        }
    }

    pub fn enqueue_reducer_effect(
        &mut self,
        reducer_name: &str,
        cap_name: &str,
        effect: &ReducerEffect,
    ) -> Result<[u8; 32], KernelError> {
        let source = EffectSource::Reducer {
            name: reducer_name.to_string(),
        };
        let runtime_kind = RuntimeEffectKind::new(effect.kind.clone());
        self.enqueue_effect(source, cap_name, runtime_kind, effect.params_cbor.clone())
    }

    pub fn drain(&mut self) -> Vec<EffectIntent> {
        self.queue.drain()
    }

    pub fn enqueue_plan_effect(
        &mut self,
        plan_name: &str,
        kind: &EffectKind,
        cap_name: &str,
        params_cbor: Vec<u8>,
    ) -> Result<[u8; 32], KernelError> {
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
    ) -> Result<[u8; 32], KernelError> {
        let grant = self
            .capability_gate
            .resolve(cap_name, runtime_kind.as_str())?;
        let intent = EffectIntent::from_raw_params(
            runtime_kind.clone(),
            cap_name.to_string(),
            params_cbor,
            [0u8; 32],
        )
        .map_err(|err| KernelError::EffectManager(err.to_string()))?;
        match self.policy_gate.decide(&intent, &grant, &source)? {
            aos_effects::traits::PolicyDecision::Allow => {
                let hash = intent.intent_hash;
                self.queue.push(intent);
                Ok(hash)
            }
            aos_effects::traits::PolicyDecision::Deny => Err(KernelError::PolicyDenied {
                effect_kind: runtime_kind.as_str().to_string(),
                origin: format_effect_origin(&source),
            }),
        }
    }
}

fn format_effect_origin(source: &EffectSource) -> String {
    match source {
        EffectSource::Reducer { name } => format!("reducer '{name}'"),
        EffectSource::Plan { name } => format!("plan '{name}'"),
    }
}
