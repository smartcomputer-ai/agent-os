use std::{collections::HashMap, path::Path};

use aos_air_types::{
    AirNode, CURRENT_AIR_VERSION, DefSecret, Manifest, NamedRef, SecretRef, builtins,
};
use aos_cbor::Hash;
use serde_json::Value as JsonValue;

use crate::store::io_error;
use crate::{EntryKind, Store, StoreError, StoreResult};

#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub hash: Hash,
    pub node: AirNode,
}

#[derive(Debug, Clone)]
pub struct Catalog {
    pub manifest: Manifest,
    pub nodes: HashMap<String, CatalogEntry>,
    pub resolved_secrets: Vec<DefSecret>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    Schema,
    Module,
    Op,
    Secret,
}

impl NodeKind {
    fn label(self) -> &'static str {
        match self {
            NodeKind::Schema => "defschema",
            NodeKind::Module => "defmodule",
            NodeKind::Op => "defop",
            NodeKind::Secret => "defsecret",
        }
    }

    fn matches(self, node: &AirNode) -> bool {
        matches!(
            (self, node),
            (NodeKind::Schema, AirNode::Defschema(_))
                | (NodeKind::Module, AirNode::Defmodule(_))
                | (NodeKind::Op, AirNode::Defop(_))
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
    load_refs(store, &manifest.ops, NodeKind::Op, &mut nodes)?;
    load_refs(store, &manifest.secrets, NodeKind::Secret, &mut nodes)?;

    let resolved_secrets = resolve_secrets(&manifest, &nodes)?;
    validate_secrets(&resolved_secrets)?;

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
                NodeKind::Schema | NodeKind::Op | NodeKind::Module => {}
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

        if kind == NodeKind::Op
            && let Some(builtin) = builtins::find_builtin_op(reference.name.as_str())
        {
            ensure_builtin_op_hash(reference, builtin)?;
            nodes.insert(
                reference.name.clone(),
                CatalogEntry {
                    hash: builtin.hash,
                    node: AirNode::Defop(builtin.op.clone()),
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

fn ensure_builtin_op_hash(reference: &NamedRef, builtin: &builtins::BuiltinOp) -> StoreResult<()> {
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
) -> StoreResult<Vec<DefSecret>> {
    let mut decls = Vec::new();
    for named in &manifest.secrets {
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
        parse_secret_name(&def.name)?;
        decls.push(def.clone());
    }
    Ok(decls)
}

fn validate_secrets(declarations: &[DefSecret]) -> StoreResult<()> {
    index_secret_decls(declarations).map(|_| ())
}

fn index_secret_decls<'a>(
    secrets: &'a [DefSecret],
) -> StoreResult<HashMap<(String, u64), &'a DefSecret>> {
    let mut map = HashMap::new();
    for secret in secrets {
        let (alias, version) = parse_secret_name(&secret.name)?;
        if secret.binding_id.trim().is_empty() {
            return Err(StoreError::SecretMissingBinding { alias, version });
        }

        let key = parse_secret_name(&secret.name)?;
        if map.insert(key.clone(), secret).is_some() {
            return Err(StoreError::DuplicateSecret {
                alias: key.0,
                version: key.1,
            });
        }
    }
    Ok(map)
}

fn resolve_secret<'a>(
    reference: &SecretRef,
    declarations: &'a HashMap<(String, u64), &'a DefSecret>,
    context: &str,
) -> StoreResult<&'a DefSecret> {
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

    Ok(decl)
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
        DefSchema, EmptyObject, HashRef, TypeExpr, TypePrimitive, TypePrimitiveText,
    };

    fn builtin_schema_refs() -> Vec<NamedRef> {
        builtins::builtin_schemas()
            .iter()
            .map(|b| NamedRef {
                name: b.schema.name.clone(),
                hash: b.hash_ref.clone(),
            })
            .collect()
    }

    fn builtin_op_refs() -> Vec<NamedRef> {
        builtins::builtin_ops()
            .iter()
            .map(|b| NamedRef {
                name: b.op.name.clone(),
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
            ops: builtin_op_refs(),
            secrets: vec![],
            routing: None,
        };

        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let catalog = load_manifest_from_bytes(&store, &manifest_bytes).expect("load");
        assert!(catalog.nodes.contains_key("com.acme/Event@1"));
    }
}
