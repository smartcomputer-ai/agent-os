use aos_air_types::{
    CapType, DefPolicy, EffectKind as AirEffectKind, OriginKind, PolicyDecision as AirDecision,
    PolicyRule,
};
use aos_effects::traits::PolicyDecision;
use aos_effects::{CapabilityGrant, EffectIntent, EffectSource};

use crate::error::KernelError;

const ALLOW_ALL_POLICY_NAME: &str = "allow_all";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDecisionDetail {
    pub policy_name: String,
    pub rule_index: Option<u32>,
    pub decision: PolicyDecision,
}

pub trait PolicyGate: Send + Sync {
    fn decide(
        &self,
        intent: &EffectIntent,
        grant: &CapabilityGrant,
        source: &EffectSource,
        cap_type: &CapType,
    ) -> Result<PolicyDecisionDetail, KernelError>;
}

#[derive(Default, Clone, Copy)]
pub struct AllowAllPolicy;

impl PolicyGate for AllowAllPolicy {
    fn decide(
        &self,
        _intent: &EffectIntent,
        _grant: &CapabilityGrant,
        _source: &EffectSource,
        _cap_type: &CapType,
    ) -> Result<PolicyDecisionDetail, KernelError> {
        Ok(PolicyDecisionDetail {
            policy_name: ALLOW_ALL_POLICY_NAME.into(),
            rule_index: None,
            decision: PolicyDecision::Allow,
        })
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
        cap_type: &CapType,
    ) -> Result<PolicyDecisionDetail, KernelError> {
        for (idx, rule) in self.rules.iter().enumerate() {
            if rule.matches(intent, source, cap_type) {
                return Ok(PolicyDecisionDetail {
                    policy_name: self.name.clone(),
                    rule_index: Some(idx as u32),
                    decision: rule.decision,
                });
            }
        }
        Ok(PolicyDecisionDetail {
            policy_name: self.name.clone(),
            rule_index: None,
            decision: PolicyDecision::Deny,
        })
    }
}

struct RuntimeRule {
    effect_kind: Option<AirEffectKind>,
    cap_name: Option<String>,
    cap_type: Option<CapType>,
    origin_kind: Option<OriginKind>,
    origin_name: Option<String>,
    decision: PolicyDecision,
}

impl RuntimeRule {
    fn from_rule(rule: &PolicyRule) -> Self {
        Self {
            effect_kind: rule.when.effect_kind.clone(),
            cap_name: rule.when.cap_name.clone(),
            cap_type: rule.when.cap_type.clone(),
            origin_kind: rule.when.origin_kind.clone(),
            origin_name: rule.when.origin_name.clone(),
            decision: match rule.decision {
                AirDecision::Allow => PolicyDecision::Allow,
                AirDecision::Deny => PolicyDecision::Deny,
            },
        }
    }

    fn matches(&self, intent: &EffectIntent, source: &EffectSource, cap_type: &CapType) -> bool {
        if let Some(expected) = &self.cap_name {
            if intent.cap_name != *expected {
                return false;
            }
        }
        if let Some(expected) = &self.cap_type {
            if expected != cap_type {
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
                &CapType::http_out(),
            )
            .unwrap();
        assert_eq!(decision.decision, PolicyDecision::Deny);
        assert_eq!(decision.rule_index, None);
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
                &CapType::http_out(),
            )
            .unwrap();
        assert_eq!(decision.decision, PolicyDecision::Deny);
        assert_eq!(decision.rule_index, Some(0));
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
                &CapType::http_out(),
            )
            .unwrap();
        assert_eq!(decision.decision, PolicyDecision::Allow);
        assert_eq!(decision.rule_index, Some(0));
    }

    #[test]
    fn cap_type_must_match_when_present() {
        let policy = DefPolicy {
            name: "com.acme/policy@3".into(),
            rules: vec![PolicyRule {
                when: PolicyMatch {
                    cap_type: Some(CapType::http_out()),
                    ..Default::default()
                },
                decision: AirDecision::Allow,
            }],
        };
        let gate = RulePolicy::from_def(&policy);
        let allowed = gate
            .decide(
                &http_intent(),
                &dummy_grant(),
                &EffectSource::Plan { name: "p".into() },
                &CapType::http_out(),
            )
            .unwrap();
        assert_eq!(allowed.decision, PolicyDecision::Allow);
        let denied = gate
            .decide(
                &http_intent(),
                &dummy_grant(),
                &EffectSource::Plan { name: "p".into() },
                &CapType::llm_basic(),
            )
            .unwrap();
        assert_eq!(denied.decision, PolicyDecision::Deny);
    }

    fn dummy_grant() -> CapabilityGrant {
        CapabilityGrant {
            name: "cap_http".into(),
            cap: "sys/http.out@1".into(),
            params_cbor: vec![],
            expiry_ns: None,
        }
    }
}
