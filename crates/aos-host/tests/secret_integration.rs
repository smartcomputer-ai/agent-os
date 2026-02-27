use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};

use aos_air_types::catalog::EffectCatalog;
use aos_air_types::{Manifest, SecretDecl, SecretEntry, SecretPolicy, CURRENT_AIR_VERSION};
use aos_cbor::Hash;
use aos_effects::builtins::LlmGenerateParams;
use aos_host::util::env_secret_resolver_from_manifest;
use aos_kernel::error::KernelError;
use aos_kernel::journal::mem::MemJournal;
use aos_kernel::secret::{
    enforce_secret_policy, inject_secrets_in_params, normalize_secret_variants, MapSecretResolver,
    SecretCatalog,
};
use aos_kernel::{Kernel, KernelConfig, LoadedManifest};
use aos_store::MemStore;
use indexmap::IndexMap;

/// SecretRef CBOR value in canonical variant form.
fn secret_ref_value(alias: &str, version: u64) -> serde_cbor::Value {
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
    Value::Map(variant)
}

/// Params CBOR for { api_key: { $tag: "secret", $value: { alias, version } } }
fn secret_param_cbor(alias: &str, version: u64) -> Vec<u8> {
    use serde_cbor::Value;
    let mut root = BTreeMap::new();
    root.insert(
        Value::Text("api_key".into()),
        secret_ref_value(alias, version),
    );
    serde_cbor::to_vec(&Value::Map(root)).unwrap()
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

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock poisoned")
}

struct EnvVarGuard {
    key: String,
    prev: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &str, value: Option<&str>) -> Self {
        let prev = std::env::var_os(key);
        match value {
            Some(val) => unsafe {
                std::env::set_var(key, val);
            },
            None => unsafe {
                std::env::remove_var(key);
            },
        }
        Self {
            key: key.to_string(),
            prev,
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.prev.as_ref() {
            Some(val) => unsafe {
                std::env::set_var(&self.key, val);
            },
            None => unsafe {
                std::env::remove_var(&self.key);
            },
        }
    }
}

fn empty_manifest() -> Manifest {
    Manifest {
        air_version: CURRENT_AIR_VERSION.to_string(),
        schemas: vec![],
        modules: vec![],
        effects: vec![],
        caps: vec![],
        policies: vec![],
        secrets: vec![],
        defaults: None,
        module_bindings: IndexMap::new(),
        routing: None,
    }
}

fn loaded_manifest_with_secret(binding_id: &str) -> LoadedManifest {
    let secret = SecretDecl {
        alias: "llm/api".into(),
        version: 1,
        binding_id: binding_id.into(),
        expected_digest: None,
        policy: None,
    };
    let mut manifest = empty_manifest();
    manifest.secrets.push(SecretEntry::Decl(secret.clone()));
    LoadedManifest {
        manifest,
        secrets: vec![secret],
        modules: HashMap::new(),
        effects: HashMap::new(),
        caps: HashMap::new(),
        policies: HashMap::new(),
        schemas: HashMap::new(),
        effect_catalog: EffectCatalog::new(),
    }
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
fn secret_policy_denies_disallowed_cap() {
    let catalog = catalog_with_secret(Some(SecretPolicy {
        allowed_caps: vec!["allowed_cap".into()],
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

    // Allowed cap passes regardless of origin identity in post-plan semantics.
    enforce_secret_policy(
        &cbor,
        &catalog,
        &aos_effects::EffectSource::Plan {
            name: "other_plan".into(),
        },
        "allowed_cap",
    )
    .expect("allowed cap should pass");
}

#[test]
fn secret_policy_denies_by_cap_only() {
    let catalog = catalog_with_secret(Some(SecretPolicy {
        allowed_caps: vec!["only_cap".into()],
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
fn secret_policy_without_cap_constraints_allows_any_origin() {
    let catalog = catalog_with_secret(Some(SecretPolicy {
        allowed_caps: vec![],
    }));
    let cbor = secret_param_cbor("llm/api", 1);
    enforce_secret_policy(
        &cbor,
        &catalog,
        &aos_effects::EffectSource::Plan {
            name: "other_plan".into(),
        },
        "any_cap",
    )
    .expect("empty allowed_caps should not deny by origin");
}

#[test]
fn env_resolver_injects_llm_api_key() {
    use serde_cbor::Value;
    let _lock = env_lock();
    let _guard = EnvVarGuard::set("AOS_TEST_LLM_API_KEY", Some("token123"));

    let loaded = loaded_manifest_with_secret("env:AOS_TEST_LLM_API_KEY");
    let resolver =
        env_secret_resolver_from_manifest(&loaded).expect("env resolver should be available");
    let catalog = SecretCatalog::new(&loaded.secrets);

    let mut params = BTreeMap::new();
    params.insert(Value::Text("provider".into()), Value::Text("openai".into()));
    params.insert(Value::Text("model".into()), Value::Text("gpt-5.2".into()));
    let mut runtime = BTreeMap::new();
    runtime.insert(Value::Text("temperature".into()), Value::Text("0.7".into()));
    runtime.insert(Value::Text("max_tokens".into()), Value::Integer(16));
    params.insert(Value::Text("runtime".into()), Value::Map(runtime));
    params.insert(
        Value::Text("message_refs".into()),
        Value::Array(vec![Value::Text(Hash::of_bytes(b"input").to_hex())]),
    );
    params.insert(
        Value::Text("api_key".into()),
        secret_ref_value("llm/api", 1),
    );
    let params_cbor = serde_cbor::to_vec(&Value::Map(params)).unwrap();

    let injected = inject_secrets_in_params(&params_cbor, &catalog, resolver.as_ref())
        .expect("inject secrets");
    let decoded: LlmGenerateParams = serde_cbor::from_slice(&injected).expect("decode params");
    assert_eq!(decoded.api_key, Some("token123".into()));
}

#[test]
fn missing_env_var_yields_secret_resolver_missing() {
    let _lock = env_lock();
    let _guard = EnvVarGuard::set("AOS_TEST_MISSING_KEY", None);

    let loaded = loaded_manifest_with_secret("env:AOS_TEST_MISSING_KEY");
    let resolver = env_secret_resolver_from_manifest(&loaded);
    assert!(
        resolver.is_none(),
        "resolver should be absent when env missing"
    );

    let store = std::sync::Arc::new(MemStore::new());
    let config = KernelConfig {
        secret_resolver: resolver,
        allow_placeholder_secrets: false,
        ..KernelConfig::default()
    };
    let result = Kernel::from_loaded_manifest_with_config(
        store,
        loaded,
        Box::new(MemJournal::new()),
        config,
    );

    assert!(matches!(result, Err(KernelError::SecretResolverMissing)));
}
