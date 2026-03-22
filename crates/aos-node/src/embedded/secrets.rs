use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aos_kernel::{LoadedManifest, MapSecretResolver, SharedSecretResolver};
use dotenvy::from_path_iter;
use serde::Deserialize;

use super::{LocalRuntimeError, LocalStatePaths};

#[derive(Debug, Deserialize)]
struct LocalSyncConfig {
    version: u32,
    #[serde(default)]
    secrets: Option<LocalSecretsSync>,
}

#[derive(Debug, Deserialize)]
struct LocalSecretsSync {
    #[serde(default)]
    sources: Vec<LocalSecretSourceSync>,
    #[serde(default)]
    bindings: Vec<LocalSecretBindingSync>,
}

#[derive(Debug, Deserialize)]
struct LocalSecretSourceSync {
    name: String,
    kind: String,
    #[serde(default)]
    path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct LocalSecretBindingSync {
    binding: String,
    from: LocalSecretBindingSourceRef,
}

#[derive(Debug, Deserialize)]
struct LocalSecretBindingSourceRef {
    source: String,
    key: String,
}

enum LoadedSecretSource {
    Dotenv { values: HashMap<String, Vec<u8>> },
    Env,
}

impl LoadedSecretSource {
    fn maybe_resolve(&self, key: &str) -> Result<Option<Vec<u8>>, LocalRuntimeError> {
        let trimmed = key.trim();
        if trimmed.is_empty() {
            return Err(LocalRuntimeError::Backend(
                "local secret source key must be non-empty".into(),
            ));
        }
        match self {
            Self::Dotenv { values } => Ok(std::env::var(trimmed)
                .ok()
                .map(|value| value.into_bytes())
                .or_else(|| values.get(trimmed).cloned())),
            Self::Env => Ok(std::env::var(trimmed).ok().map(|value| value.into_bytes())),
        }
    }
}

pub fn local_secret_resolver_for_manifest(
    paths: &LocalStatePaths,
    loaded: &LoadedManifest,
) -> Result<Option<SharedSecretResolver>, LocalRuntimeError> {
    if loaded.secrets.is_empty() {
        return Ok(None);
    }

    let required_bindings = loaded
        .secrets
        .iter()
        .map(|secret| secret.binding_id.clone())
        .collect::<BTreeSet<_>>();
    let mut values = HashMap::new();

    if let Some(world_root) = paths.world_root() {
        values.extend(load_local_secret_value_map(world_root, &required_bindings)?);
    }

    for binding in &required_bindings {
        let Some(var_name) = binding.strip_prefix("env:") else {
            continue;
        };
        if var_name.is_empty() {
            return Err(LocalRuntimeError::Backend(format!(
                "invalid empty env binding '{binding}'"
            )));
        }
        if let Ok(value) = std::env::var(var_name) {
            values.insert(binding.clone(), value.into_bytes());
        }
    }

    Ok(Some(Arc::new(MapSecretResolver::new(values))))
}

fn load_local_secret_value_map(
    world_root: &Path,
    required_bindings: &BTreeSet<String>,
) -> Result<HashMap<String, Vec<u8>>, LocalRuntimeError> {
    let sync_path = world_root.join("aos.sync.json");
    if !sync_path.exists() {
        return Ok(HashMap::new());
    }

    let bytes = std::fs::read(&sync_path)?;
    let config: LocalSyncConfig = serde_json::from_slice(&bytes)?;
    if config.version != 1 {
        return Err(LocalRuntimeError::Backend(format!(
            "unsupported local sync config version {} in {}",
            config.version,
            sync_path.display()
        )));
    }

    let Some(secrets) = config.secrets else {
        return Ok(HashMap::new());
    };
    let map_root = sync_path.parent().unwrap_or(world_root);

    let mut sources = HashMap::new();
    for source in &secrets.sources {
        let name = source.name.trim();
        if name.is_empty() {
            return Err(LocalRuntimeError::Backend(
                "local sync secret source name must be non-empty".into(),
            ));
        }
        let loaded = load_secret_source(map_root, source)?;
        if sources.insert(name.to_string(), loaded).is_some() {
            return Err(LocalRuntimeError::Backend(format!(
                "duplicate local sync secret source '{name}'"
            )));
        }
    }

    let mut seen_bindings = HashSet::new();
    let mut values = HashMap::new();
    for binding in &secrets.bindings {
        let binding_id = binding.binding.trim();
        if binding_id.is_empty() {
            return Err(LocalRuntimeError::Backend(
                "local sync secret binding id must be non-empty".into(),
            ));
        }
        if !seen_bindings.insert(binding_id.to_string()) {
            return Err(LocalRuntimeError::Backend(format!(
                "duplicate local sync secret binding '{binding_id}'"
            )));
        }
        if !required_bindings.contains(binding_id) {
            continue;
        }
        let source = sources.get(binding.from.source.as_str()).ok_or_else(|| {
            LocalRuntimeError::Backend(format!(
                "local sync secret binding '{binding_id}' references unknown source '{}'",
                binding.from.source
            ))
        })?;
        if let Some(value) = source.maybe_resolve(binding.from.key.as_str())? {
            values.insert(binding_id.to_string(), value);
        }
    }

    Ok(values)
}

fn load_secret_source(
    map_root: &Path,
    source: &LocalSecretSourceSync,
) -> Result<LoadedSecretSource, LocalRuntimeError> {
    let kind = source.kind.trim();
    if kind.is_empty() {
        return Err(LocalRuntimeError::Backend(format!(
            "local sync secret source '{}' must set kind",
            source.name
        )));
    }

    match kind {
        "dotenv" => {
            let path = source
                .path
                .as_ref()
                .map(|path| resolve_map_path(map_root, path))
                .unwrap_or_else(|| map_root.join(".env"));
            let mut values = HashMap::new();
            if path.exists() {
                for item in from_path_iter(&path).map_err(|err| {
                    LocalRuntimeError::Backend(format!(
                        "load dotenv secret source {}: {err}",
                        path.display()
                    ))
                })? {
                    let (key, value) = item.map_err(|err| {
                        LocalRuntimeError::Backend(format!(
                            "parse dotenv secret source {}: {err}",
                            path.display()
                        ))
                    })?;
                    values.insert(key, value.into_bytes());
                }
            }
            Ok(LoadedSecretSource::Dotenv { values })
        }
        "env" => Ok(LoadedSecretSource::Env),
        other => Err(LocalRuntimeError::Backend(format!(
            "unsupported local sync secret source kind '{}' for source '{}'",
            other, source.name
        ))),
    }
}

fn resolve_map_path(root: &Path, path: &Path) -> PathBuf {
    if path.is_relative() {
        root.join(path)
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::{
        CURRENT_AIR_VERSION, Manifest, SecretDecl, SecretEntry, catalog::EffectCatalog,
    };
    use indexmap::IndexMap;

    fn loaded_manifest_with_secret(binding_id: &str) -> LoadedManifest {
        let secret = SecretDecl {
            alias: "llm/api".into(),
            version: 1,
            binding_id: binding_id.into(),
            expected_digest: None,
            policy: None,
        };
        LoadedManifest {
            manifest: Manifest {
                air_version: CURRENT_AIR_VERSION.to_string(),
                schemas: vec![],
                modules: vec![],
                effects: vec![],
                effect_bindings: vec![],
                caps: vec![],
                policies: vec![],
                secrets: vec![SecretEntry::Decl(secret.clone())],
                defaults: None,
                module_bindings: IndexMap::new(),
                routing: None,
            },
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
    fn local_secret_resolver_reads_dotenv_binding() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("aos.sync.json"),
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "secrets": {
                    "sources": [{ "name": "local_env", "kind": "dotenv", "path": ".env" }],
                    "bindings": [{
                        "binding": "llm/openai_api",
                        "from": { "source": "local_env", "key": "OPENAI_API_KEY" }
                    }]
                }
            }))
            .expect("encode sync config"),
        )
        .expect("write sync config");
        std::fs::write(temp.path().join(".env"), "OPENAI_API_KEY=from-dotenv\n")
            .expect("write .env");

        let resolver = local_secret_resolver_for_manifest(
            &LocalStatePaths::from_world_root(temp.path()),
            &loaded_manifest_with_secret("llm/openai_api"),
        )
        .expect("resolver")
        .expect("resolver present");
        let value = resolver
            .resolve("llm/openai_api", 1, None)
            .expect("resolve binding");
        assert_eq!(value.value, b"from-dotenv");
    }

    #[test]
    fn local_secret_resolver_allows_missing_values() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("aos.sync.json"),
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "secrets": {
                    "sources": [{ "name": "local_env", "kind": "dotenv", "path": ".env" }],
                    "bindings": [{
                        "binding": "llm/openai_api",
                        "from": { "source": "local_env", "key": "OPENAI_API_KEY" }
                    }]
                }
            }))
            .expect("encode sync config"),
        )
        .expect("write sync config");

        let resolver = local_secret_resolver_for_manifest(
            &LocalStatePaths::from_world_root(temp.path()),
            &loaded_manifest_with_secret("llm/openai_api"),
        )
        .expect("resolver")
        .expect("resolver present");
        assert!(resolver.resolve("llm/openai_api", 1, None).is_err());
    }
}
