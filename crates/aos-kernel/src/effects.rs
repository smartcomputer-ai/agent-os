use aos_air_types::EffectKind;
use aos_effects::EffectIntent;
use aos_wasm_abi::ReducerEffect;

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
}

impl EffectManager {
    pub fn new() -> Self {
        Self {
            queue: EffectQueue::default(),
        }
    }

    pub fn enqueue_reducer_effects(
        &mut self,
        effects: &[ReducerEffect],
    ) -> Result<(), KernelError> {
        for eff in effects {
            let cap_name = eff.cap_slot.clone().unwrap_or_else(|| "default".into());
            let intent = EffectIntent::from_raw_params(
                eff.kind.clone().into(),
                cap_name,
                eff.params_cbor.clone(),
                [0u8; 32],
            )
            .map_err(|err| KernelError::EffectManager(err.to_string()))?;
            self.queue.push(intent);
        }
        Ok(())
    }

    pub fn drain(&mut self) -> Vec<EffectIntent> {
        self.queue.drain()
    }

    pub fn enqueue_plan_effect(
        &mut self,
        kind: &EffectKind,
        cap_name: &str,
        params_cbor: Vec<u8>,
    ) -> Result<[u8; 32], KernelError> {
        let intent = EffectIntent::from_raw_params(
            aos_effects::EffectKind::from_air(kind.clone()),
            cap_name.to_string(),
            params_cbor,
            [0u8; 32],
        )
        .map_err(|err| KernelError::EffectManager(err.to_string()))?;
        let hash = intent.intent_hash;
        self.queue.push(intent);
        Ok(hash)
    }
}
