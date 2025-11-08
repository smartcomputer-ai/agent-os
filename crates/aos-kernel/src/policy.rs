use aos_effects::traits::PolicyDecision;
use aos_effects::{CapabilityGrant, EffectIntent, EffectSource};

use crate::error::KernelError;

pub trait PolicyGate {
    fn decide(
        &self,
        intent: &EffectIntent,
        grant: &CapabilityGrant,
        source: &EffectSource,
    ) -> Result<PolicyDecision, KernelError>;
}

#[derive(Default, Clone, Copy)]
pub struct AllowAllPolicy;

impl PolicyGate for AllowAllPolicy {
    fn decide(
        &self,
        _intent: &EffectIntent,
        _grant: &CapabilityGrant,
        _source: &EffectSource,
    ) -> Result<PolicyDecision, KernelError> {
        Ok(PolicyDecision::Allow)
    }
}
