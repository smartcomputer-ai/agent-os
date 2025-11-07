use crate::{CapabilityGrant, EffectIntent, EffectReceipt, EffectSource, ReceiptStatus};

/// Result of a policy decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny,
}

/// Capability gate resolves and validates capability grants before dispatch.
pub trait CapabilityGate {
    type Error;

    fn resolve(&self, cap_name: &str, effect_kind: &str) -> Result<CapabilityGrant, Self::Error>;
    fn check_constraints(
        &self,
        intent: &EffectIntent,
        grant: &CapabilityGrant,
    ) -> Result<(), Self::Error>;
}

/// Policy gate evaluates origin/effect metadata for allow/deny flows.
pub trait PolicyGate {
    type Error;

    fn decide(
        &self,
        intent: &EffectIntent,
        grant: &CapabilityGrant,
        source: &EffectSource,
    ) -> Result<PolicyDecision, Self::Error>;
}

/// Adapter trait executed by the effect manager; async runtimes can wrap it as needed.
pub trait EffectAdapter {
    type Error;

    fn kind(&self) -> &str;
    fn execute(&self, intent: &EffectIntent) -> Result<EffectReceipt, Self::Error>;
}

/// Helper describing the desired receipt status for waiting plans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptExpectation {
    pub accept: ReceiptStatus,
}
