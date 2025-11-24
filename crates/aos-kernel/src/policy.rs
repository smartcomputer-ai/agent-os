use aos_air_types::{
    DefPolicy, EffectKind as AirEffectKind, OriginKind, PolicyDecision as AirDecision, PolicyRule,
};
use aos_effects::traits::PolicyDecision;
use aos_effects::{CapabilityGrant, EffectIntent, EffectSource};

use crate::error::KernelError;

pub trait PolicyGate: Send + Sync {
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

pub struct RulePolicy {
    name: String,
    rules: Vec<RuntimeRule>,
}

impl RulePolicy {
    pub fn from_def(policy: &DefPolicy) -> Self {
        let rules = policy
            .rules
            .iter()
            .map(|rule| RuntimeRule::from_rule(rule))
            .collect();
        Self {
            name: policy.name.clone(),
            rules,
        }
    }
}

impl PolicyGate for RulePolicy {
    fn decide(
        &self,
        intent: &EffectIntent,
        _grant: &CapabilityGrant,
        source: &EffectSource,
    ) -> Result<PolicyDecision, KernelError> {
        for rule in &self.rules {
            if rule.matches(intent, source) {
                return Ok(rule.decision);
            }
        }
        Ok(PolicyDecision::Deny)
    }
}

struct RuntimeRule {
    effect_kind: Option<AirEffectKind>,
    cap_name: Option<String>,
    origin_kind: Option<OriginKind>,
    origin_name: Option<String>,
    decision: PolicyDecision,
    host_or_method_specified: bool,
}

impl RuntimeRule {
    fn from_rule(rule: &PolicyRule) -> Self {
        Self {
            effect_kind: rule.when.effect_kind.clone(),
            cap_name: rule.when.cap_name.clone(),
            origin_kind: rule.when.origin_kind.clone(),
            origin_name: rule.when.origin_name.clone(),
            decision: match rule.decision {
                AirDecision::Allow => PolicyDecision::Allow,
                AirDecision::Deny => PolicyDecision::Deny,
            },
            host_or_method_specified: rule.when.host.is_some() || rule.when.method.is_some(),
        }
    }

    fn matches(&self, intent: &EffectIntent, source: &EffectSource) -> bool {
        if self.host_or_method_specified {
            return false;
        }
        if let Some(expected) = &self.cap_name {
            if intent.cap_name != *expected {
                return false;
            }
        }
        if let Some(kind) = &self.effect_kind {
            if !effect_kind_matches(kind, intent.kind.as_str()) {
                return false;
            }
        }
        if let Some(kind) = &self.origin_kind {
            if !origin_kind_matches(kind, source) {
                return false;
            }
        }
        if let Some(name) = &self.origin_name {
            if source.origin_name() != name {
                return false;
            }
        }
        true
    }
}

fn effect_kind_matches(matcher: &AirEffectKind, actual: &str) -> bool {
    matcher.as_str() == actual
}

fn origin_kind_matches(expected: &OriginKind, source: &EffectSource) -> bool {
    match (expected, source) {
        (OriginKind::Plan, EffectSource::Plan { .. }) => true,
        (OriginKind::Reducer, EffectSource::Reducer { .. }) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::{EffectKind as AirEffectKind, PolicyMatch, PolicyRule};
    use aos_effects::{EffectIntent, EffectKind, EffectSource};

    fn http_intent() -> EffectIntent {
        EffectIntent::from_raw_params(
            EffectKind::new(EffectKind::HTTP_REQUEST),
            "cap_http",
            vec![],
            [0u8; 32],
        )
        .unwrap()
    }

    #[test]
    fn default_deny_when_no_rule_matches() {
        let policy = DefPolicy {
            name: "com.acme/none@1".into(),
            rules: vec![],
        };
        let gate = RulePolicy::from_def(&policy);
        let decision = gate
            .decide(
                &http_intent(),
                &dummy_grant(),
                &EffectSource::Reducer { name: "r".into() },
            )
            .unwrap();
        assert_eq!(decision, PolicyDecision::Deny);
    }

    #[test]
    fn reducer_http_can_be_denied() {
        let policy = DefPolicy {
            name: "com.acme/policy@1".into(),
            rules: vec![PolicyRule {
                when: PolicyMatch {
                    effect_kind: Some(AirEffectKind::new(AirEffectKind::HTTP_REQUEST)),
                    origin_kind: Some(OriginKind::Reducer),
                    ..Default::default()
                },
                decision: AirDecision::Deny,
            }],
        };
        let gate = RulePolicy::from_def(&policy);
        let decision = gate
            .decide(
                &http_intent(),
                &dummy_grant(),
                &EffectSource::Reducer { name: "r".into() },
            )
            .unwrap();
        assert_eq!(decision, PolicyDecision::Deny);
    }

    #[test]
    fn plan_allowed_by_rule() {
        let policy = DefPolicy {
            name: "com.acme/policy@2".into(),
            rules: vec![PolicyRule {
                when: PolicyMatch {
                    effect_kind: Some(AirEffectKind::new(AirEffectKind::HTTP_REQUEST)),
                    origin_kind: Some(OriginKind::Plan),
                    ..Default::default()
                },
                decision: AirDecision::Allow,
            }],
        };
        let gate = RulePolicy::from_def(&policy);
        let decision = gate
            .decide(
                &http_intent(),
                &dummy_grant(),
                &EffectSource::Plan { name: "p".into() },
            )
            .unwrap();
        assert_eq!(decision, PolicyDecision::Allow);
    }

    fn dummy_grant() -> CapabilityGrant {
        CapabilityGrant {
            name: "cap_http".into(),
            cap: "sys/http.out@1".into(),
            params_cbor: vec![],
            expiry_ns: None,
            budget: None,
        }
    }
}
