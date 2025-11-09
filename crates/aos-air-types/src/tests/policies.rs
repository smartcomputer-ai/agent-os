use serde_json::json;
use std::panic::{self, AssertUnwindSafe};

use super::assert_json_schema;
use crate::{DefPolicy, PolicyDecision, PolicyMatch, PolicyRule};

#[test]
fn parses_policy_rules() {
    let policy_json = json!({
        "$kind": "defpolicy",
        "name": "com.acme/policy@1",
        "rules": [
            {
                "when": {"effect_kind": "http.request", "origin_kind": "plan"},
                "decision": "allow"
            }
        ]
    });
    assert_json_schema(crate::schemas::DEFPOLICY, &policy_json);
    let policy: DefPolicy = serde_json::from_value(policy_json).expect("policy json");
    assert_eq!(policy.rules.len(), 1);
    assert!(matches!(policy.rules[0].decision, PolicyDecision::Allow));
}

#[test]
fn rule_without_decision_errors() {
    let bad_rule = json!({
        "when": {"effect_kind": "http.request"}
    });
    assert!(serde_json::from_value::<PolicyRule>(bad_rule).is_err());
}

#[test]
fn policy_match_serializes_round_trip() {
    let r#match = PolicyMatch {
        effect_kind: Some(crate::EffectKind::HttpRequest),
        cap_name: None,
        host: Some("example.com".into()),
        method: Some("GET".into()),
        origin_kind: Some(crate::OriginKind::Plan),
        origin_name: Some("com.acme/Plan@1".into()),
    };
    let json = serde_json::to_value(&r#match).expect("serialize");
    let round_trip: PolicyMatch = serde_json::from_value(json).expect("deserialize");
    assert_eq!(round_trip.method.as_deref(), Some("GET"));
}

#[test]
fn policy_supports_multiple_rules_and_filters() {
    let policy_json = json!({
        "$kind": "defpolicy",
        "name": "com.acme/policy@2",
        "rules": [
            {
                "when": {
                    "effect_kind": "http.request",
                    "cap_name": "cap_http",
                    "host": "example.com",
                    "method": "GET",
                    "origin_kind": "plan",
                    "origin_name": "com.acme/Plan@1"
                },
                "decision": "allow"
            },
            {
                "when": {
                    "effect_kind": "llm.generate"
                },
                "decision": "deny"
            }
        ]
    });
    assert_json_schema(crate::schemas::DEFPOLICY, &policy_json);
    let policy: DefPolicy = serde_json::from_value(policy_json).expect("policy json");
    assert_eq!(policy.rules.len(), 2);
    assert!(matches!(policy.rules[1].decision, PolicyDecision::Deny));
}

#[test]
fn rule_without_when_is_rejected_by_schema() {
    let rule = json!({ "decision": "allow" });
    assert!(serde_json::from_value::<PolicyRule>(rule.clone()).is_err());
    let policy_json = json!({
        "$kind": "defpolicy",
        "name": "com.acme/policy@3",
        "rules": [rule]
    });
    assert!(
        panic::catch_unwind(AssertUnwindSafe(|| assert_json_schema(
            crate::schemas::DEFPOLICY,
            &policy_json
        )))
        .is_err()
    );
}
