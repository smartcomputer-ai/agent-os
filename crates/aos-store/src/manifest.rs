use std::{collections::HashMap, path::Path};

use aos_air_types::{
    AirNode, CURRENT_AIR_VERSION, CapGrant, DefPlan, ExprOrValue, Manifest, NamedRef, PlanStepKind,
    SecretDecl, SecretEntry, SecretPolicy, SecretRef, ValueLiteral, builtins,
    plan_literals::SchemaIndex, validate,
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
    Plan,
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
            NodeKind::Plan => "defplan",
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
                | (NodeKind::Plan, AirNode::Defplan(_))
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
    load_refs(store, &manifest.plans, NodeKind::Plan, &mut nodes)?;
    load_refs(store, &manifest.effects, NodeKind::Effect, &mut nodes)?;
    load_refs(store, &manifest.caps, NodeKind::Cap, &mut nodes)?;
    load_refs(store, &manifest.policies, NodeKind::Policy, &mut nodes)?;
    load_secret_refs(store, &manifest.secrets, &mut nodes)?;

    normalize_plan_literals(&mut nodes)?;
    let resolved_secrets = resolve_secrets(&manifest, &nodes)?;
    validate_plans(&manifest, &nodes, &resolved_secrets)?;
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
        if kind == NodeKind::Schema {
            if let Some(builtin) = builtins::find_builtin_schema(reference.name.as_str()) {
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
        }
        if kind == NodeKind::Effect {
            if let Some(builtin) = builtins::find_builtin_effect(reference.name.as_str()) {
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
    let mut refs = Vec::new();
    for entry in secrets {
        if let SecretEntry::Ref(named) = entry {
            refs.push(named.clone());
        }
    }
    if refs.is_empty() {
        return Ok(());
    }
    load_refs(store, &refs, NodeKind::Secret, nodes)
}

fn normalize_plan_literals(nodes: &mut HashMap<String, CatalogEntry>) -> StoreResult<()> {
    use aos_air_types::plan_literals::normalize_plan_literals;

    let mut schema_map = HashMap::new();
    let mut module_map = HashMap::new();
    let mut effect_defs = Vec::new();
    for entry in nodes.values() {
        match &entry.node {
            AirNode::Defschema(schema) => {
                schema_map.insert(schema.name.clone(), schema.ty.clone());
            }
            AirNode::Defmodule(module) => {
                module_map.insert(module.name.clone(), module.clone());
            }
            AirNode::Defeffect(effect) => effect_defs.push(effect.clone()),
            _ => {}
        }
    }
    for builtin in builtins::builtin_schemas() {
        schema_map
            .entry(builtin.schema.name.clone())
            .or_insert(builtin.schema.ty.clone());
    }
    let schema_index = SchemaIndex::new(schema_map);
    let effect_catalog = aos_air_types::catalog::EffectCatalog::from_defs(effect_defs);
    for (name, entry) in nodes.iter_mut() {
        if let AirNode::Defplan(plan) = &mut entry.node {
            normalize_plan_literals(plan, &schema_index, &module_map, &effect_catalog).map_err(
                |source| StoreError::PlanNormalization {
                    name: name.clone(),
                    source,
                },
            )?;
        }
    }
    Ok(())
}

fn parse_hash_str(value: &str) -> StoreResult<Hash> {
    Hash::from_hex_str(value).map_err(|source| StoreError::InvalidHashString {
        value: value.to_string(),
        source,
    })
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
        return Err(StoreError::InvalidSecretName {
            name: name.to_string(),
            reason: "version must be >= 1".into(),
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
                        allowed_plans: def.allowed_plans.clone(),
                    })
                    .filter(|p| !p.allowed_caps.is_empty() || !p.allowed_plans.is_empty()),
                });
            }
        }
    }
    Ok(decls)
}

fn validate_plans(
    manifest: &Manifest,
    nodes: &HashMap<String, CatalogEntry>,
    secrets: &[SecretDecl],
) -> StoreResult<()> {
    for plan_ref in &manifest.plans {
        if let Some(entry) = nodes.get(&plan_ref.name) {
            if let AirNode::Defplan(plan) = &entry.node {
                validate::validate_plan(plan).map_err(|source| StoreError::PlanValidation {
                    name: plan.name.clone(),
                    source,
                })?;
                validate_plan_secrets(plan, &index_secret_decls(secrets)?)?;
            }
        }
    }
    Ok(())
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

fn validate_plan_secrets(
    plan: &DefPlan,
    declarations: &HashMap<(String, u64), &SecretDecl>,
) -> StoreResult<()> {
    let mut refs = Vec::new();
    for step in &plan.steps {
        match &step.kind {
            PlanStepKind::RaiseEvent(step) => {
                collect_secret_refs_in_expr_or_value(&step.event, &mut refs);
            }
            PlanStepKind::EmitEffect(step) => {
                collect_secret_refs_in_expr_or_value(&step.params, &mut refs);
            }
            PlanStepKind::Assign(step) => {
                collect_secret_refs_in_expr_or_value(&step.expr, &mut refs);
            }
            PlanStepKind::End(step) => {
                if let Some(result) = &step.result {
                    collect_secret_refs_in_expr_or_value(result, &mut refs);
                }
            }
            _ => {}
        }
    }

    for reference in refs {
        resolve_secret(
            &reference,
            declarations,
            &format!("plan {}", plan.name),
            Some(plan.name.as_str()),
            None,
        )?;
    }

    Ok(())
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
            None,
            Some(grant.name.as_str()),
        )?;
    }
    Ok(())
}

fn resolve_secret<'a>(
    reference: &SecretRef,
    declarations: &'a HashMap<(String, u64), &'a SecretDecl>,
    context: &str,
    plan_name: Option<&str>,
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

    if let Some(policy) = decl.policy.as_ref() {
        if let Some(plan) = plan_name {
            if !policy.allowed_plans.is_empty() && !policy.allowed_plans.iter().any(|p| p == plan) {
                return Err(StoreError::SecretPolicyViolation {
                    alias: decl.alias.clone(),
                    version: decl.version,
                    context: context.to_string(),
                });
            }
        }
        if let Some(cap) = cap_name {
            if !policy.allowed_caps.is_empty() && !policy.allowed_caps.iter().any(|c| c == cap) {
                return Err(StoreError::SecretPolicyViolation {
                    alias: decl.alias.clone(),
                    version: decl.version,
                    context: context.to_string(),
                });
            }
        }
    }

    Ok(decl)
}

fn collect_secret_refs_in_expr_or_value(value: &ExprOrValue, refs: &mut Vec<SecretRef>) {
    match value {
        ExprOrValue::Literal(literal) => collect_secret_refs_in_value_literal(literal, refs),
        ExprOrValue::Json(json) => collect_secret_refs_in_json(json, refs),
        ExprOrValue::Expr(_) => {}
    }
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
        AirNode, CapGrant, CapType, DefCap, DefPlan, DefSecret, EffectKind, EmptyObject, Expr,
        ExprConst, ExprOp, ExprOpCode, ExprRef, HashRef, Manifest, ManifestDefaults, NamedRef,
        PlanBind, PlanBindEffect, PlanEdge, PlanStep, PlanStepAssign, PlanStepAwaitReceipt,
        PlanStepEmitEffect, PlanStepEnd, PlanStepKind, SchemaRef, SecretDecl, SecretEntry,
        SecretPolicy, SecretRef, TypeExpr, TypePrimitive, TypePrimitiveText, TypeRef, ValueLiteral,
        ValueMap, ValueNull, ValueRecord, ValueText,
    };
    use indexmap::IndexMap;

    fn sample_plan() -> DefPlan {
        let expr = Expr::Const(ExprConst::Text {
            text: "payload".into(),
        });
        let http_params = ValueLiteral::Record(ValueRecord {
            record: IndexMap::from([
                (
                    "method".into(),
                    ValueLiteral::Text(ValueText { text: "GET".into() }),
                ),
                (
                    "url".into(),
                    ValueLiteral::Text(ValueText {
                        text: "https://example.com".into(),
                    }),
                ),
                (
                    "headers".into(),
                    ValueLiteral::Map(ValueMap { map: vec![] }),
                ),
                (
                    "body_ref".into(),
                    ValueLiteral::Null(ValueNull {
                        null: EmptyObject::default(),
                    }),
                ),
            ]),
        });

        DefPlan {
            name: "com.acme/plan@1".into(),
            input: SchemaRef::new("com.acme/Input@1").unwrap(),
            output: None,
            locals: IndexMap::new(),
            steps: vec![
                PlanStep {
                    id: "emit".into(),
                    kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                        kind: EffectKind::http_request(),
                        params: http_params.clone().into(),
                        cap: "http_cap".into(),
                        bind: PlanBindEffect {
                            effect_id_as: "req".into(),
                        },
                    }),
                },
                PlanStep {
                    id: "await".into(),
                    kind: PlanStepKind::AwaitReceipt(PlanStepAwaitReceipt {
                        for_expr: Expr::Ref(ExprRef {
                            reference: "@var:req".into(),
                        }),
                        bind: PlanBind { var: "rcpt".into() },
                    }),
                },
                PlanStep {
                    id: "assign".into(),
                    kind: PlanStepKind::Assign(PlanStepAssign {
                        expr: Expr::Op(ExprOp {
                            op: ExprOpCode::Concat,
                            args: vec![
                                expr.clone(),
                                Expr::Const(ExprConst::Text { text: "!".into() }),
                            ],
                        })
                        .into(),
                        bind: PlanBind {
                            var: "result".into(),
                        },
                    }),
                },
                PlanStep {
                    id: "end".into(),
                    kind: PlanStepKind::End(PlanStepEnd { result: None }),
                },
            ],
            edges: vec![
                PlanEdge {
                    from: "emit".into(),
                    to: "await".into(),
                    when: None,
                },
                PlanEdge {
                    from: "await".into(),
                    to: "assign".into(),
                    when: None,
                },
                PlanEdge {
                    from: "assign".into(),
                    to: "end".into(),
                    when: None,
                },
            ],
            required_caps: vec!["http_cap".into()],
            allowed_effects: vec![EffectKind::http_request()],
            invariants: vec![],
        }
    }

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

    fn empty_manifest_with_builtins() -> Manifest {
        Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: builtin_schema_refs(),
            modules: vec![],
            plans: vec![],
            effects: builtin_effect_refs(),
            caps: vec![],
            policies: vec![],
            secrets: vec![],
            defaults: None,
            module_bindings: IndexMap::new(),
            routing: None,
            triggers: vec![],
        }
    }

    fn manifest_with_plan(plan_ref: NamedRef) -> Manifest {
        let mut manifest = empty_manifest_with_builtins();
        manifest.plans.push(plan_ref);
        manifest
    }

    #[test]
    fn load_manifest_success() {
        let store = MemStore::default();
        let plan = sample_plan();
        let plan_hash = store
            .put_node(&AirNode::Defplan(plan.clone()))
            .expect("store plan");
        let manifest = manifest_with_plan(NamedRef {
            name: plan.name.clone(),
            hash: HashRef::new(plan_hash.to_hex()).unwrap(),
        });
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let catalog = load_manifest_from_bytes(&store, &manifest_bytes).expect("load");
        assert!(catalog.nodes.contains_key(&plan.name));
    }

    #[test]
    fn detects_node_kind_mismatch() {
        let store = MemStore::default();
        let schema_node = AirNode::Defschema(aos_air_types::DefSchema {
            name: "com.acme/Schema@1".into(),
            ty: TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                text: EmptyObject::default(),
            })),
        });
        let hash = store.put_node(&schema_node).expect("store schema");
        let manifest = manifest_with_plan(NamedRef {
            name: "com.acme/plan@1".into(),
            hash: HashRef::new(hash.to_hex()).unwrap(),
        });
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::NodeKindMismatch { .. }));
    }

    #[test]
    fn plan_validation_failure_propagates() {
        let store = MemStore::default();
        let mut plan = sample_plan();
        plan.steps.push(plan.steps[0].clone()); // duplicate id triggers validation error
        let hash = store
            .put_node(&AirNode::Defplan(plan.clone()))
            .expect("store plan");
        let manifest = manifest_with_plan(NamedRef {
            name: plan.name.clone(),
            hash: HashRef::new(hash.to_hex()).unwrap(),
        });
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::PlanValidation { .. }));
    }

    #[test]
    #[test]
    fn rejects_unknown_secret_reference() {
        let store = MemStore::default();
        let defcap = DefCap {
            name: "com.acme/secret-cap@1".into(),
            cap_type: CapType::secret(),
            schema: TypeExpr::Ref(TypeRef {
                reference: SchemaRef::new("sys/SecretRef@1").unwrap(),
            }),
        };
        let defcap_hash = store
            .put_node(&AirNode::Defcap(defcap.clone()))
            .expect("store defcap");
        let cap_grant = CapGrant {
            name: "secret_cap".into(),
            cap: defcap.name.clone(),
            params: ValueLiteral::SecretRef(SecretRef {
                alias: "payments/stripe".into(),
                version: 1,
            }),
            expiry_ns: None,
            budget: None,
        };
        let secret_schema = builtins::find_builtin_schema("sys/SecretRef@1").unwrap();
        let mut manifest = empty_manifest_with_builtins();
        manifest.schemas.push(NamedRef {
            name: secret_schema.schema.name.clone(),
            hash: secret_schema.hash_ref.clone(),
        });
        manifest.caps.push(NamedRef {
            name: defcap.name.clone(),
            hash: HashRef::new(defcap_hash.to_hex()).unwrap(),
        });
        manifest.defaults = Some(ManifestDefaults {
            policy: None,
            cap_grants: vec![cap_grant],
        });
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(
            matches!(err, StoreError::UnknownSecret { .. }),
            "expected UnknownSecret, got {err:?}"
        );
    }

    #[test]
    fn rejects_secret_version_zero() {
        let store = MemStore::default();
        let defcap = DefCap {
            name: "com.acme/secret-cap@1".into(),
            cap_type: CapType::secret(),
            schema: TypeExpr::Ref(TypeRef {
                reference: SchemaRef::new("sys/SecretRef@1").unwrap(),
            }),
        };
        let defcap_hash = store
            .put_node(&AirNode::Defcap(defcap.clone()))
            .expect("store defcap");
        let cap_grant = CapGrant {
            name: "secret_cap".into(),
            cap: defcap.name.clone(),
            params: ValueLiteral::SecretRef(SecretRef {
                alias: "payments/stripe".into(),
                version: 0,
            }),
            expiry_ns: None,
            budget: None,
        };
        let secret_schema = builtins::find_builtin_schema("sys/SecretRef@1").unwrap();
        let mut manifest = empty_manifest_with_builtins();
        manifest.schemas.push(NamedRef {
            name: secret_schema.schema.name.clone(),
            hash: secret_schema.hash_ref.clone(),
        });
        manifest.caps.push(NamedRef {
            name: defcap.name.clone(),
            hash: HashRef::new(defcap_hash.to_hex()).unwrap(),
        });
        manifest.defaults = Some(ManifestDefaults {
            policy: None,
            cap_grants: vec![cap_grant],
        });
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::InvalidSecretVersion { .. }));
    }

    #[test]
    fn rejects_duplicate_secret_decl() {
        let store = MemStore::default();
        let mut manifest = empty_manifest_with_builtins();
        manifest.secrets = vec![
            SecretEntry::Decl(SecretDecl {
                alias: "payments/stripe".into(),
                version: 1,
                binding_id: "stripe:prod".into(),
                expected_digest: None,
                policy: None,
            }),
            SecretEntry::Decl(SecretDecl {
                alias: "payments/stripe".into(),
                version: 1,
                binding_id: "stripe:prod".into(),
                expected_digest: None,
                policy: None,
            }),
        ];
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::DuplicateSecret { .. }));
    }

    #[test]
    fn rejects_secret_missing_binding() {
        let store = MemStore::default();
        let mut manifest = empty_manifest_with_builtins();
        manifest.secrets.push(SecretEntry::Decl(SecretDecl {
            alias: "payments/stripe".into(),
            version: 1,
            binding_id: " ".into(),
            expected_digest: None,
            policy: None,
        }));
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::SecretMissingBinding { .. }));
    }

    #[test]
    fn rejects_secret_policy_for_cap() {
        let store = MemStore::default();
        let mut manifest = empty_manifest_with_builtins();
        manifest.secrets.push(SecretEntry::Decl(SecretDecl {
            alias: "payments/stripe".into(),
            version: 1,
            binding_id: "stripe:prod".into(),
            expected_digest: None,
            policy: Some(SecretPolicy {
                allowed_caps: vec!["other_cap".into()],
                allowed_plans: vec![],
            }),
        }));
        manifest.defaults = Some(ManifestDefaults {
            policy: None,
            cap_grants: vec![CapGrant {
                name: "secret_cap".into(),
                cap: "com.acme/secret@1".into(),
                params: ValueLiteral::SecretRef(SecretRef {
                    alias: "payments/stripe".into(),
                    version: 1,
                }),
                expiry_ns: None,
                budget: None,
            }],
        });
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::SecretPolicyViolation { .. }));
    }

    #[test]
    fn rejects_duplicate_defsecret_alias_version() {
        let store = MemStore::default();
        let def = DefSecret {
            name: "payments/stripe@1".into(),
            binding_id: "binding".into(),
            expected_digest: None,
            allowed_caps: vec![],
            allowed_plans: vec![],
        };
        let hash = store.put_node(&AirNode::Defsecret(def.clone())).unwrap();
        // Two references to the same alias/version via the same defsecret hash
        let mut manifest = empty_manifest_with_builtins();
        manifest.secrets = vec![
            SecretEntry::Ref(NamedRef {
                name: def.name.clone(),
                hash: HashRef::new(hash.to_hex()).unwrap(),
            }),
            SecretEntry::Ref(NamedRef {
                name: def.name.clone(),
                hash: HashRef::new(hash.to_hex()).unwrap(),
            }),
        ];
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::DuplicateSecret { .. }));
    }

    #[test]
    fn rejects_defsecret_missing_binding() {
        let store = MemStore::default();
        let def = DefSecret {
            name: "payments/stripe@1".into(),
            binding_id: " ".into(),
            expected_digest: None,
            allowed_caps: vec![],
            allowed_plans: vec![],
        };
        let hash = store.put_node(&AirNode::Defsecret(def)).unwrap();
        let mut manifest = empty_manifest_with_builtins();
        manifest.secrets.push(SecretEntry::Ref(NamedRef {
            name: "payments/stripe@1".into(),
            hash: HashRef::new(hash.to_hex()).unwrap(),
        }));
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::SecretMissingBinding { .. }));
    }

    #[test]
    fn missing_air_version_is_rejected() {
        let store = MemStore::default();
        use serde_cbor::Value;
        use std::collections::BTreeMap;
        let mut map: BTreeMap<Value, Value> = BTreeMap::new();
        map.insert(Value::Text("$kind".into()), Value::Text("manifest".into()));
        map.insert(Value::Text("schemas".into()), Value::Array(vec![]));
        map.insert(Value::Text("modules".into()), Value::Array(vec![]));
        map.insert(Value::Text("plans".into()), Value::Array(vec![]));
        map.insert(Value::Text("effects".into()), Value::Array(vec![]));
        map.insert(Value::Text("caps".into()), Value::Array(vec![]));
        map.insert(Value::Text("policies".into()), Value::Array(vec![]));
        map.insert(Value::Text("secrets".into()), Value::Array(vec![]));
        map.insert(
            Value::Text("module_bindings".into()),
            Value::Map(BTreeMap::new()),
        );
        map.insert(Value::Text("triggers".into()), Value::Array(vec![]));
        let manifest_bytes = serde_cbor::to_vec(&Value::Map(map)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::MissingAirVersion { .. }));
    }

    #[test]
    fn loads_defsecret_refs() {
        let store = MemStore::default();
        let def = DefSecret {
            name: "payments/stripe@1".into(),
            binding_id: "binding".into(),
            expected_digest: None,
            allowed_caps: vec![],
            allowed_plans: vec![],
        };
        let hash = store.put_node(&AirNode::Defsecret(def)).unwrap();
        let mut manifest = empty_manifest_with_builtins();
        manifest.secrets.push(SecretEntry::Ref(NamedRef {
            name: "payments/stripe@1".into(),
            hash: HashRef::new(hash.to_hex()).unwrap(),
        }));
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let catalog = load_manifest_from_bytes(&store, &manifest_bytes).expect("load manifest");
        assert_eq!(catalog.resolved_secrets.len(), 1);
        assert_eq!(catalog.resolved_secrets[0].alias, "payments/stripe");
        assert_eq!(catalog.resolved_secrets[0].version, 1);
    }

    #[test]
    fn rejects_invalid_defsecret_name() {
        let store = MemStore::default();
        let def = DefSecret {
            name: "payments-stripe-nover".into(),
            binding_id: "binding".into(),
            expected_digest: None,
            allowed_caps: vec![],
            allowed_plans: vec![],
        };
        let hash = store.put_node(&AirNode::Defsecret(def)).unwrap();
        let mut manifest = empty_manifest_with_builtins();
        manifest.secrets.push(SecretEntry::Ref(NamedRef {
            name: "payments-stripe-nover".into(),
            hash: HashRef::new(hash.to_hex()).unwrap(),
        }));
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::InvalidSecretName { .. }));
    }
}
