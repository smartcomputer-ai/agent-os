use std::collections::{BTreeMap, HashMap};

use aos_air_types::{SecretDecl, SecretPolicy};
use aos_kernel::error::KernelError;
use aos_kernel::secret::{
    MapSecretResolver, SecretCatalog, enforce_secret_policy, inject_secrets_in_params,
    normalize_secret_variants,
};

/// Params CBOR for { api_key: { $tag: "secret", $value: { alias, version } } }
fn secret_param_cbor(alias: &str, version: u64) -> Vec<u8> {
    use serde_cbor::Value;
    let mut inner = BTreeMap::new();
    inner.insert(Value::Text("alias".into()), Value::Text(alias.into()));
    inner.insert(
        Value::Text("version".into()),
        Value::Integer(version as i128),
    );
    let mut variant = BTreeMap::new();
    variant.insert(Value::Text("$tag".into()), Value::Text("secret".into()));
    variant.insert(Value::Text("$value".into()), Value::Map(inner));
    let mut root = BTreeMap::new();
    root.insert(Value::Text("api_key".into()), Value::Map(variant));
    Value::Map(root)
        .to_owned()
        .try_into()
        .map(|v: Value| serde_cbor::to_vec(&v).unwrap())
        .unwrap()
}

fn catalog_with_secret(policy: Option<SecretPolicy>) -> SecretCatalog {
    let decl = SecretDecl {
        alias: "llm/api".into(),
        version: 1,
        binding_id: "env:LLM_API_KEY".into(),
        expected_digest: None,
        policy,
    };
    SecretCatalog::new(&[decl])
}

#[test]
fn injects_secret_into_params_cbor() {
    let catalog = catalog_with_secret(None);
    let mut map = HashMap::new();
    map.insert("env:LLM_API_KEY".into(), b"token123".to_vec());
    let resolver = MapSecretResolver::new(map);

    let cbor = secret_param_cbor("llm/api", 1);
    let injected =
        inject_secrets_in_params(&cbor, &catalog, &resolver).expect("inject secrets succeeds");
    let value: serde_cbor::Value = serde_cbor::from_slice(&injected).unwrap();
    if let serde_cbor::Value::Map(root) = value {
        let api_val = root
            .get(&serde_cbor::Value::Text("api_key".into()))
            .unwrap();
        assert_eq!(
            api_val,
            &serde_cbor::Value::Text("token123".into()),
            "secret should be injected as plaintext"
        );
    } else {
        panic!("expected map at root");
    }
}

#[test]
fn injects_non_utf8_secret_as_bytes() {
    let catalog = catalog_with_secret(None);
    let mut map = HashMap::new();
    map.insert("env:LLM_API_KEY".into(), vec![0xFF, 0x00, 0x01]);
    let resolver = MapSecretResolver::new(map);
    let cbor = secret_param_cbor("llm/api", 1);
    let injected = inject_secrets_in_params(&cbor, &catalog, &resolver).unwrap();
    let value: serde_cbor::Value = serde_cbor::from_slice(&injected).unwrap();
    if let serde_cbor::Value::Map(root) = value {
        let api_val = root
            .get(&serde_cbor::Value::Text("api_key".into()))
            .unwrap();
        assert_eq!(api_val, &serde_cbor::Value::Bytes(vec![0xFF, 0x00, 0x01]));
    } else {
        panic!("expected map root");
    }
}

#[test]
fn normalizes_secret_sugar_to_canonical_variant() {
    use serde_cbor::Value;
    // {"api_key": {"secret": {"alias": "llm/api", "version": 1}}}
    let mut inner = BTreeMap::new();
    inner.insert(Value::Text("alias".into()), Value::Text("llm/api".into()));
    inner.insert(Value::Text("version".into()), Value::Integer(1));
    let mut sugar = BTreeMap::new();
    sugar.insert(Value::Text("secret".into()), Value::Map(inner));
    let mut root = BTreeMap::new();
    root.insert(Value::Text("api_key".into()), Value::Map(sugar));
    let sugar_cbor = serde_cbor::to_vec(&Value::Map(root)).unwrap();

    let normalized = normalize_secret_variants(&sugar_cbor).expect("normalize");
    let value: Value = serde_cbor::from_slice(&normalized).unwrap();
    if let Value::Map(root) = value {
        let api_val = root.get(&Value::Text("api_key".into())).unwrap();
        match api_val {
            Value::Map(m) => {
                assert_eq!(
                    m.get(&Value::Text("$tag".into())),
                    Some(&Value::Text("secret".into()))
                );
                assert!(m.contains_key(&Value::Text("$value".into())));
            }
            other => panic!("expected variant map, got {:?}", other),
        }
    } else {
        panic!("expected map at root");
    }
}

#[test]
fn expected_digest_mismatch_fails() {
    // expected digest for "token123"
    let expected =
        aos_air_types::HashRef::new(aos_cbor::Hash::of_bytes(b"token123").to_hex()).unwrap();
    let decl = SecretDecl {
        alias: "llm/api".into(),
        version: 1,
        binding_id: "env:LLM_API_KEY".into(),
        expected_digest: Some(expected),
        policy: None,
    };
    let catalog = SecretCatalog::new(&[decl]);
    let resolver = MapSecretResolver::new(HashMap::from([(
        "env:LLM_API_KEY".into(),
        b"wrong".to_vec(),
    )]));
    let cbor = secret_param_cbor("llm/api", 1);
    let err = inject_secrets_in_params(&cbor, &catalog, &resolver).unwrap_err();
    assert!(matches!(
        err,
        aos_kernel::secret::SecretResolverError::DigestMismatch { .. }
    ));
}

#[test]
fn secret_policy_denies_disallowed_cap_or_plan() {
    let catalog = catalog_with_secret(Some(SecretPolicy {
        allowed_caps: vec!["allowed_cap".into()],
        allowed_plans: vec!["allowed_plan".into()],
    }));
    let _resolver = MapSecretResolver::new(HashMap::from([(
        "env:LLM_API_KEY".into(),
        b"token123".to_vec(),
    )]));
    let cbor = secret_param_cbor("llm/api", 1);
    // Policy should run on original params
    let err = enforce_secret_policy(
        &cbor,
        &catalog,
        &aos_effects::EffectSource::Plan {
            name: "allowed_plan".into(),
        },
        "other_cap",
    )
    .unwrap_err();
    assert!(matches!(err, KernelError::SecretPolicyDenied { .. }));

    // Plan not allowlisted
    let err = enforce_secret_policy(
        &cbor,
        &catalog,
        &aos_effects::EffectSource::Plan {
            name: "other_plan".into(),
        },
        "allowed_cap",
    )
    .unwrap_err();
    assert!(matches!(err, KernelError::SecretPolicyDenied { .. }));
}

#[test]
fn secret_policy_denies_by_cap_only() {
    let catalog = catalog_with_secret(Some(SecretPolicy {
        allowed_caps: vec!["only_cap".into()],
        allowed_plans: vec![],
    }));
    let cbor = secret_param_cbor("llm/api", 1);
    // Different cap should be denied regardless of plan
    let err = enforce_secret_policy(
        &cbor,
        &catalog,
        &aos_effects::EffectSource::Plan {
            name: "any_plan".into(),
        },
        "other_cap",
    )
    .unwrap_err();
    assert!(matches!(err, KernelError::SecretPolicyDenied { .. }));
}

#[test]
fn secret_policy_denies_by_plan_only() {
    let catalog = catalog_with_secret(Some(SecretPolicy {
        allowed_caps: vec![],
        allowed_plans: vec!["only_plan".into()],
    }));
    let cbor = secret_param_cbor("llm/api", 1);
    let err = enforce_secret_policy(
        &cbor,
        &catalog,
        &aos_effects::EffectSource::Plan {
            name: "other_plan".into(),
        },
        "any_cap",
    )
    .unwrap_err();
    assert!(matches!(err, KernelError::SecretPolicyDenied { .. }));
}
