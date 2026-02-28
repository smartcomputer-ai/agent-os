use std::{collections::HashMap, path::Path};

use aos_air_types::{
    AirNode, CURRENT_AIR_VERSION, CapGrant, Manifest, NamedRef, SecretDecl, SecretEntry,
    SecretPolicy, SecretRef, ValueLiteral, builtins,
};
use aos_cbor::Hash;
use serde_json::Value as JsonValue;

use crate::{EntryKind, Store, StoreError, StoreResult, io_error};

#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub hash: Hash,
    pub node: AirNode,
}

#[derive(Debug, Clone)]
pub struct Catalog {
    pub manifest: Manifest,
    pub nodes: HashMap<String, CatalogEntry>,
    pub resolved_secrets: Vec<SecretDecl>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    Schema,
    Module,
    Effect,
    Cap,
    Policy,
    Secret,
}

impl NodeKind {
    fn label(self) -> &'static str {
        match self {
            NodeKind::Schema => "defschema",
            NodeKind::Module => "defmodule",
            NodeKind::Effect => "defeffect",
            NodeKind::Cap => "defcap",
            NodeKind::Policy => "defpolicy",
            NodeKind::Secret => "defsecret",
        }
    }

    fn matches(self, node: &AirNode) -> bool {
        matches!(
            (self, node),
            (NodeKind::Schema, AirNode::Defschema(_))
                | (NodeKind::Module, AirNode::Defmodule(_))
                | (NodeKind::Effect, AirNode::Defeffect(_))
                | (NodeKind::Cap, AirNode::Defcap(_))
                | (NodeKind::Policy, AirNode::Defpolicy(_))
                | (NodeKind::Secret, AirNode::Defsecret(_))
        )
    }
}

pub fn load_manifest_from_path<S: Store>(
    store: &S,
    path: impl AsRef<Path>,
) -> StoreResult<Catalog> {
    let path_ref = path.as_ref();
    let bytes = std::fs::read(path_ref).map_err(|e| io_error(path_ref, e))?;
    load_manifest_from_bytes(store, &bytes)
}

pub fn load_manifest_from_bytes<S: Store>(store: &S, bytes: &[u8]) -> StoreResult<Catalog> {
    let value: serde_cbor::Value = serde_cbor::from_slice(bytes)?;
    if !has_air_version_field(&value) {
        return Err(StoreError::MissingAirVersion {
            supported: CURRENT_AIR_VERSION.to_string(),
        });
    }
    let manifest: Manifest = serde_cbor::value::from_value(value)?;
    ensure_air_version(&manifest)?;

    let mut nodes = HashMap::new();
    load_refs(store, &manifest.schemas, NodeKind::Schema, &mut nodes)?;
    load_refs(store, &manifest.modules, NodeKind::Module, &mut nodes)?;
    load_refs(store, &manifest.effects, NodeKind::Effect, &mut nodes)?;
    load_refs(store, &manifest.caps, NodeKind::Cap, &mut nodes)?;
    load_refs(store, &manifest.policies, NodeKind::Policy, &mut nodes)?;
    load_secret_refs(store, &manifest.secrets, &mut nodes)?;
    insert_builtin_caps(&mut nodes);

    let resolved_secrets = resolve_secrets(&manifest, &nodes)?;
    validate_secrets(&manifest, &resolved_secrets)?;

    Ok(Catalog {
        manifest,
        nodes,
        resolved_secrets,
    })
}

fn has_air_version_field(value: &serde_cbor::Value) -> bool {
    if let serde_cbor::Value::Map(map) = value {
        return map
            .iter()
            .any(|(k, _)| matches!(k, serde_cbor::Value::Text(s) if s == "air_version"));
    }
    false
}

fn ensure_air_version(manifest: &Manifest) -> StoreResult<()> {
    if manifest.air_version == CURRENT_AIR_VERSION {
        Ok(())
    } else {
        Err(StoreError::UnsupportedAirVersion {
            found: manifest.air_version.clone(),
            supported: CURRENT_AIR_VERSION.to_string(),
        })
    }
}

fn load_refs<S: Store>(
    store: &S,
    refs: &[NamedRef],
    kind: NodeKind,
    nodes: &mut HashMap<String, CatalogEntry>,
) -> StoreResult<()> {
    for reference in refs {
        if is_sys_name(reference.name.as_str()) {
            match kind {
                NodeKind::Schema | NodeKind::Effect | NodeKind::Cap | NodeKind::Module => {}
                _ => {
                    return Err(StoreError::ReservedSysName {
                        kind: kind.label(),
                        name: reference.name.clone(),
                    });
                }
            }
        }

        if kind == NodeKind::Schema
            && let Some(builtin) = builtins::find_builtin_schema(reference.name.as_str())
        {
            ensure_builtin_hash(reference, builtin)?;
            nodes.insert(
                reference.name.clone(),
                CatalogEntry {
                    hash: builtin.hash,
                    node: AirNode::Defschema(builtin.schema.clone()),
                },
            );
            continue;
        }

        if kind == NodeKind::Module
            && let Some(builtin) = builtins::find_builtin_module(reference.name.as_str())
        {
            if reference.hash.as_str().is_empty() || reference.hash == builtin.hash_ref {
                nodes.insert(
                    reference.name.clone(),
                    CatalogEntry {
                        hash: builtin.hash,
                        node: AirNode::Defmodule(builtin.module.clone()),
                    },
                );
                continue;
            }
            let hash = parse_hash_str(reference.hash.as_str())?;
            let node: AirNode = store.get_node(hash)?;
            if !kind.matches(&node) {
                return Err(StoreError::NodeKindMismatch {
                    name: reference.name.clone(),
                    expected: kind.label(),
                });
            }
            nodes.insert(reference.name.clone(), CatalogEntry { hash, node });
            continue;
        }

        if kind == NodeKind::Effect
            && let Some(builtin) = builtins::find_builtin_effect(reference.name.as_str())
        {
            ensure_builtin_effect_hash(reference, builtin)?;
            nodes.insert(
                reference.name.clone(),
                CatalogEntry {
                    hash: builtin.hash,
                    node: AirNode::Defeffect(builtin.effect.clone()),
                },
            );
            continue;
        }

        if kind == NodeKind::Cap
            && let Some(builtin) = builtins::find_builtin_cap(reference.name.as_str())
        {
            ensure_builtin_cap_hash(reference, builtin)?;
            nodes.insert(
                reference.name.clone(),
                CatalogEntry {
                    hash: builtin.hash,
                    node: AirNode::Defcap(builtin.cap.clone()),
                },
            );
            continue;
        }

        if is_sys_name(reference.name.as_str()) {
            return Err(StoreError::ReservedSysName {
                kind: kind.label(),
                name: reference.name.clone(),
            });
        }

        let hash = parse_hash_str(reference.hash.as_str())?;
        let node: AirNode = store.get_node(hash)?;
        if !kind.matches(&node) {
            return Err(StoreError::NodeKindMismatch {
                name: reference.name.clone(),
                expected: kind.label(),
            });
        }
        nodes.insert(reference.name.clone(), CatalogEntry { hash, node });
    }
    Ok(())
}

fn load_secret_refs<S: Store>(
    store: &S,
    secrets: &[SecretEntry],
    nodes: &mut HashMap<String, CatalogEntry>,
) -> StoreResult<()> {
    let refs: Vec<NamedRef> = secrets
        .iter()
        .filter_map(|entry| match entry {
            SecretEntry::Ref(named) => Some(named.clone()),
            SecretEntry::Decl(_) => None,
        })
        .collect();
    if refs.is_empty() {
        return Ok(());
    }
    load_refs(store, &refs, NodeKind::Secret, nodes)
}

fn insert_builtin_caps(nodes: &mut HashMap<String, CatalogEntry>) {
    for builtin in builtins::builtin_caps() {
        nodes
            .entry(builtin.cap.name.clone())
            .or_insert_with(|| CatalogEntry {
                hash: builtin.hash,
                node: AirNode::Defcap(builtin.cap.clone()),
            });
    }
}

fn parse_hash_str(value: &str) -> StoreResult<Hash> {
    Hash::from_hex_str(value).map_err(|source| StoreError::InvalidHashString {
        value: value.to_string(),
        source,
    })
}

fn is_sys_name(name: &str) -> bool {
    name.starts_with("sys/")
}

fn parse_secret_name(name: &str) -> StoreResult<(String, u64)> {
    let parts: Vec<&str> = name.rsplitn(2, '@').collect();
    if parts.len() != 2 {
        return Err(StoreError::InvalidSecretName {
            name: name.to_string(),
            reason: "missing @version suffix".into(),
        });
    }
    let version = parts[0]
        .parse::<u64>()
        .map_err(|_| StoreError::InvalidSecretName {
            name: name.to_string(),
            reason: "version is not a positive integer".into(),
        })?;
    if version < 1 {
        return Err(StoreError::InvalidSecretVersion {
            alias: parts[1].to_string(),
            version,
            context: "defsecret name".into(),
        });
    }
    Ok((parts[1].to_string(), version))
}

fn ensure_builtin_hash(reference: &NamedRef, builtin: &builtins::BuiltinSchema) -> StoreResult<()> {
    let actual = parse_hash_str(reference.hash.as_str())?;
    if actual != builtin.hash {
        return Err(StoreError::HashMismatch {
            kind: EntryKind::Node,
            expected: builtin.hash,
            actual,
        });
    }
    Ok(())
}

fn ensure_builtin_effect_hash(
    reference: &NamedRef,
    builtin: &builtins::BuiltinEffect,
) -> StoreResult<()> {
    let actual = parse_hash_str(reference.hash.as_str())?;
    if actual != builtin.hash {
        return Err(StoreError::HashMismatch {
            kind: EntryKind::Node,
            expected: builtin.hash,
            actual,
        });
    }
    Ok(())
}

fn ensure_builtin_cap_hash(
    reference: &NamedRef,
    builtin: &builtins::BuiltinCap,
) -> StoreResult<()> {
    let actual = parse_hash_str(reference.hash.as_str())?;
    if actual != builtin.hash {
        return Err(StoreError::HashMismatch {
            kind: EntryKind::Node,
            expected: builtin.hash,
            actual,
        });
    }
    Ok(())
}

fn resolve_secrets(
    manifest: &Manifest,
    nodes: &HashMap<String, CatalogEntry>,
) -> StoreResult<Vec<SecretDecl>> {
    let mut decls = Vec::new();
    for entry in &manifest.secrets {
        match entry {
            SecretEntry::Decl(decl) => decls.push(decl.clone()),
            SecretEntry::Ref(named) => {
                let Some(node) = nodes.get(&named.name) else {
                    return Err(StoreError::UnknownSecret {
                        alias: named.name.clone(),
                        version: 0,
                        context: "defsecret not loaded".into(),
                    });
                };
                let AirNode::Defsecret(def) = &node.node else {
                    return Err(StoreError::NodeKindMismatch {
                        name: named.name.clone(),
                        expected: NodeKind::Secret.label(),
                    });
                };
                let (alias, version) = parse_secret_name(&def.name)?;
                decls.push(SecretDecl {
                    alias,
                    version,
                    binding_id: def.binding_id.clone(),
                    expected_digest: def.expected_digest.clone(),
                    policy: Some(SecretPolicy {
                        allowed_caps: def.allowed_caps.clone(),
                    })
                    .filter(|p| !p.allowed_caps.is_empty()),
                });
            }
        }
    }
    Ok(decls)
}

fn validate_secrets(manifest: &Manifest, declarations: &[SecretDecl]) -> StoreResult<()> {
    let declarations = index_secret_decls(declarations)?;
    if let Some(defaults) = manifest.defaults.as_ref() {
        for grant in &defaults.cap_grants {
            validate_cap_grant_secrets(grant, &declarations)?;
        }
    }
    Ok(())
}

fn index_secret_decls<'a>(
    secrets: &'a [SecretDecl],
) -> StoreResult<HashMap<(String, u64), &'a SecretDecl>> {
    let mut map = HashMap::new();
    for secret in secrets {
        if secret.binding_id.trim().is_empty() {
            return Err(StoreError::SecretMissingBinding {
                alias: secret.alias.clone(),
                version: secret.version,
            });
        }

        let key = (secret.alias.clone(), secret.version);
        if map.insert(key.clone(), secret).is_some() {
            return Err(StoreError::DuplicateSecret {
                alias: key.0,
                version: key.1,
            });
        }
    }
    Ok(map)
}

fn validate_cap_grant_secrets(
    grant: &CapGrant,
    declarations: &HashMap<(String, u64), &SecretDecl>,
) -> StoreResult<()> {
    let mut refs = Vec::new();
    collect_secret_refs_in_value_literal(&grant.params, &mut refs);
    for reference in refs {
        resolve_secret(
            &reference,
            declarations,
            &format!("cap grant {}", grant.name),
            Some(grant.name.as_str()),
        )?;
    }
    Ok(())
}

fn resolve_secret<'a>(
    reference: &SecretRef,
    declarations: &'a HashMap<(String, u64), &'a SecretDecl>,
    context: &str,
    cap_name: Option<&str>,
) -> StoreResult<&'a SecretDecl> {
    if reference.version < 1 {
        return Err(StoreError::InvalidSecretVersion {
            alias: reference.alias.clone(),
            version: reference.version,
            context: context.to_string(),
        });
    }

    let Some(decl) = declarations.get(&(reference.alias.clone(), reference.version)) else {
        return Err(StoreError::UnknownSecret {
            alias: reference.alias.clone(),
            version: reference.version,
            context: context.to_string(),
        });
    };

    if let Some(policy) = decl.policy.as_ref()
        && let Some(cap) = cap_name
        && !policy.allowed_caps.is_empty()
        && !policy.allowed_caps.iter().any(|c| c == cap)
    {
        return Err(StoreError::SecretPolicyViolation {
            alias: decl.alias.clone(),
            version: decl.version,
            context: context.to_string(),
        });
    }

    Ok(decl)
}

fn collect_secret_refs_in_value_literal(value: &ValueLiteral, refs: &mut Vec<SecretRef>) {
    match value {
        ValueLiteral::SecretRef(secret) => refs.push(secret.clone()),
        ValueLiteral::List(list) => {
            for item in &list.list {
                collect_secret_refs_in_value_literal(item, refs);
            }
        }
        ValueLiteral::Set(set) => {
            for item in &set.set {
                collect_secret_refs_in_value_literal(item, refs);
            }
        }
        ValueLiteral::Map(map) => {
            for entry in &map.map {
                collect_secret_refs_in_value_literal(&entry.key, refs);
                collect_secret_refs_in_value_literal(&entry.value, refs);
            }
        }
        ValueLiteral::Record(record) => {
            for field in record.record.values() {
                collect_secret_refs_in_value_literal(field, refs);
            }
        }
        ValueLiteral::Variant(variant) => {
            if let Some(value) = variant.value.as_deref() {
                collect_secret_refs_in_value_literal(value, refs);
            }
        }
        _ => {}
    }
}

#[allow(dead_code)]
fn collect_secret_refs_in_json(value: &JsonValue, refs: &mut Vec<SecretRef>) {
    if let Some(secret) = try_parse_json_secret_ref(value) {
        refs.push(secret);
    }

    match value {
        JsonValue::Array(values) => {
            for item in values {
                collect_secret_refs_in_json(item, refs);
            }
        }
        JsonValue::Object(map) => {
            for item in map.values() {
                collect_secret_refs_in_json(item, refs);
            }
        }
        _ => {}
    }
}

#[allow(dead_code)]
fn try_parse_json_secret_ref(value: &JsonValue) -> Option<SecretRef> {
    let JsonValue::Object(map) = value else {
        return None;
    };

    let alias = map.get("alias")?.as_str()?;
    let version = map.get("version")?.as_u64()?;

    if map.len() != 2 {
        return None;
    }

    Some(SecretRef {
        alias: alias.to_string(),
        version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemStore;
    use aos_air_types::{
        DefSchema, EmptyObject, HashRef, ManifestDefaults, TypeExpr, TypePrimitive,
        TypePrimitiveText, ValueRecord,
    };
    use indexmap::IndexMap;

    fn builtin_schema_refs() -> Vec<NamedRef> {
        builtins::builtin_schemas()
            .iter()
            .map(|b| NamedRef {
                name: b.schema.name.clone(),
                hash: b.hash_ref.clone(),
            })
            .collect()
    }

    fn builtin_effect_refs() -> Vec<NamedRef> {
        builtins::builtin_effects()
            .iter()
            .map(|b| NamedRef {
                name: b.effect.name.clone(),
                hash: b.hash_ref.clone(),
            })
            .collect()
    }

    #[test]
    fn load_manifest_success_without_plans() {
        let store = MemStore::default();
        let schema = DefSchema {
            name: "com.acme/Event@1".into(),
            ty: TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                text: EmptyObject::default(),
            })),
        };
        let schema_hash = store.put_node(&AirNode::Defschema(schema.clone())).unwrap();

        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: {
                let mut refs = builtin_schema_refs();
                refs.push(NamedRef {
                    name: schema.name.clone(),
                    hash: HashRef::new(schema_hash.to_hex()).unwrap(),
                });
                refs
            },
            modules: vec![],
            effects: builtin_effect_refs(),
            effect_bindings: vec![],

            caps: vec![],
            policies: vec![],
            secrets: vec![],
            defaults: None,
            module_bindings: IndexMap::new(),
            routing: None,
        };

        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let catalog = load_manifest_from_bytes(&store, &manifest_bytes).expect("load");
        assert!(catalog.nodes.contains_key("com.acme/Event@1"));
    }

    #[test]
    fn cap_grant_secret_policy_is_enforced() {
        let store = MemStore::default();
        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: builtin_schema_refs(),
            modules: vec![],
            effects: builtin_effect_refs(),
            effect_bindings: vec![],

            caps: vec![],
            policies: vec![],
            secrets: vec![SecretEntry::Decl(SecretDecl {
                alias: "api_key".into(),
                version: 1,
                binding_id: "secret/api_key".into(),
                expected_digest: None,
                policy: Some(SecretPolicy {
                    allowed_caps: vec!["allowed_cap".into()],
                }),
            })],
            defaults: Some(ManifestDefaults {
                policy: None,
                cap_grants: vec![CapGrant {
                    name: "blocked_cap".into(),
                    cap: "sys/cap/http_public@1".into(),
                    params: ValueLiteral::Record(ValueRecord {
                        record: IndexMap::from([(
                            "api_key".into(),
                            ValueLiteral::SecretRef(SecretRef {
                                alias: "api_key".into(),
                                version: 1,
                            }),
                        )]),
                    }),
                    expiry_ns: None,
                }],
            }),
            module_bindings: IndexMap::new(),
            routing: None,
        };

        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::SecretPolicyViolation { .. }));
    }
}
