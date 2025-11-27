use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use aos_air_types::{
    AirNode, CapGrant, DefPlan, ExprOrValue, Manifest, NamedRef, PlanStepKind, SecretDecl,
    SecretRef, ValueLiteral, builtins, plan_literals::SchemaIndex, validate,
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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    Schema,
    Module,
    Plan,
    Cap,
    Policy,
}

impl NodeKind {
    fn label(self) -> &'static str {
        match self {
            NodeKind::Schema => "defschema",
            NodeKind::Module => "defmodule",
            NodeKind::Plan => "defplan",
            NodeKind::Cap => "defcap",
            NodeKind::Policy => "defpolicy",
        }
    }

    fn matches(self, node: &AirNode) -> bool {
        matches!(
            (self, node),
            (NodeKind::Schema, AirNode::Defschema(_))
                | (NodeKind::Module, AirNode::Defmodule(_))
                | (NodeKind::Plan, AirNode::Defplan(_))
                | (NodeKind::Cap, AirNode::Defcap(_))
                | (NodeKind::Policy, AirNode::Defpolicy(_))
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
    let mut manifest: Manifest = serde_cbor::from_slice(bytes)?;
    ensure_builtin_schema_refs(&mut manifest)?;
    let mut nodes = HashMap::new();

    load_refs(store, &manifest.schemas, NodeKind::Schema, &mut nodes)?;
    load_refs(store, &manifest.modules, NodeKind::Module, &mut nodes)?;
    load_refs(store, &manifest.plans, NodeKind::Plan, &mut nodes)?;
    load_refs(store, &manifest.caps, NodeKind::Cap, &mut nodes)?;
    load_refs(store, &manifest.policies, NodeKind::Policy, &mut nodes)?;

    normalize_plan_literals(&mut nodes)?;
    validate_plans(&manifest, &nodes)?;
    validate_secrets(&manifest, &nodes)?;

    Ok(Catalog { manifest, nodes })
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

fn normalize_plan_literals(nodes: &mut HashMap<String, CatalogEntry>) -> StoreResult<()> {
    use aos_air_types::plan_literals::normalize_plan_literals;

    let mut schema_map = HashMap::new();
    let mut module_map = HashMap::new();
    for entry in nodes.values() {
        match &entry.node {
            AirNode::Defschema(schema) => {
                schema_map.insert(schema.name.clone(), schema.ty.clone());
            }
            AirNode::Defmodule(module) => {
                module_map.insert(module.name.clone(), module.clone());
            }
            _ => {}
        }
    }
    for builtin in builtins::builtin_schemas() {
        schema_map
            .entry(builtin.schema.name.clone())
            .or_insert(builtin.schema.ty.clone());
    }
    let schema_index = SchemaIndex::new(schema_map);
    for (name, entry) in nodes.iter_mut() {
        if let AirNode::Defplan(plan) = &mut entry.node {
            normalize_plan_literals(plan, &schema_index, &module_map).map_err(|source| {
                StoreError::PlanNormalization {
                    name: name.clone(),
                    source,
                }
            })?;
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

fn ensure_builtin_schema_refs(manifest: &mut Manifest) -> StoreResult<()> {
    let mut present: HashSet<String> = HashSet::new();
    for reference in &manifest.schemas {
        if let Some(builtin) = builtins::find_builtin_schema(reference.name.as_str()) {
            ensure_builtin_hash(reference, builtin)?;
            present.insert(reference.name.clone());
        }
    }

    for name in referenced_builtin_schema_names(manifest) {
        if present.contains(&name) {
            continue;
        }
        if let Some(builtin) = builtins::find_builtin_schema(&name) {
            manifest.schemas.push(NamedRef {
                name: builtin.schema.name.clone(),
                hash: builtin.hash_ref.clone(),
            });
            present.insert(name);
        }
    }
    Ok(())
}

fn referenced_builtin_schema_names(manifest: &Manifest) -> HashSet<String> {
    let mut names = HashSet::new();
    if let Some(routing) = manifest.routing.as_ref() {
        for route in &routing.events {
            maybe_add_builtin_name(route.event.as_str(), &mut names);
        }
    }
    for trigger in &manifest.triggers {
        maybe_add_builtin_name(trigger.event.as_str(), &mut names);
    }
    names
}

fn maybe_add_builtin_name(schema: &str, names: &mut HashSet<String>) {
    if builtins::find_builtin_schema(schema).is_some() {
        names.insert(schema.to_string());
    }
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

fn validate_plans(manifest: &Manifest, nodes: &HashMap<String, CatalogEntry>) -> StoreResult<()> {
    for plan_ref in &manifest.plans {
        if let Some(entry) = nodes.get(&plan_ref.name) {
            if let AirNode::Defplan(plan) = &entry.node {
                validate::validate_plan(plan).map_err(|source| StoreError::PlanValidation {
                    name: plan.name.clone(),
                    source,
                })?;
            }
        }
    }
    Ok(())
}

fn validate_secrets(manifest: &Manifest, nodes: &HashMap<String, CatalogEntry>) -> StoreResult<()> {
    let declarations = index_secret_decls(&manifest.secrets)?;
    for plan_ref in &manifest.plans {
        if let Some(entry) = nodes.get(&plan_ref.name) {
            if let AirNode::Defplan(plan) = &entry.node {
                validate_plan_secrets(plan, &declarations)?;
            }
        }
    }

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
        AirNode, CapGrant, CapType, DefCap, DefPlan, EffectKind, EmptyObject, Expr, ExprConst,
        ExprOp, ExprOpCode, ExprRef, HashRef, Manifest, ManifestDefaults, NamedRef, PlanBind,
        PlanBindEffect, PlanEdge, PlanStep, PlanStepAssign, PlanStepAwaitReceipt,
        PlanStepEmitEffect, PlanStepEnd, PlanStepKind, Routing, RoutingEvent, SchemaRef,
        SecretDecl, SecretPolicy, SecretRef, TypeExpr, TypePrimitive, TypePrimitiveText, TypeRef,
        ValueLiteral, ValueMap, ValueNull, ValueRecord, ValueText,
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

    fn manifest_with_plan(plan_ref: NamedRef) -> Manifest {
        Manifest {
            schemas: vec![],
            modules: vec![],
            plans: vec![plan_ref],
            caps: vec![],
            policies: vec![],
            secrets: vec![],
            defaults: None,
            module_bindings: IndexMap::new(),
            routing: None,
            triggers: vec![],
        }
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
    fn injects_builtin_schema_for_routed_events() {
        let store = MemStore::default();
        let manifest = Manifest {
            schemas: vec![],
            modules: vec![],
            plans: vec![],
            caps: vec![],
            policies: vec![],
            secrets: vec![],
            defaults: None,
            module_bindings: IndexMap::new(),
            routing: Some(Routing {
                events: vec![RoutingEvent {
                    event: SchemaRef::new("sys/TimerFired@1").unwrap(),
                    reducer: "com.acme/Reducer@1".into(),
                    key_field: None,
                }],
                inboxes: vec![],
            }),
            triggers: vec![],
        };
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let catalog = load_manifest_from_bytes(&store, &manifest_bytes).expect("load");
        assert!(
            catalog
                .manifest
                .schemas
                .iter()
                .any(|r| r.name == "sys/TimerFired@1")
        );
        assert!(matches!(
            catalog
                .nodes
                .get("sys/TimerFired@1")
                .map(|entry| &entry.node),
            Some(AirNode::Defschema(_))
        ));
    }

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
        let manifest = Manifest {
            schemas: vec![NamedRef {
                name: secret_schema.schema.name.clone(),
                hash: secret_schema.hash_ref.clone(),
            }],
            modules: vec![],
            plans: vec![],
            caps: vec![NamedRef {
                name: defcap.name.clone(),
                hash: HashRef::new(defcap_hash.to_hex()).unwrap(),
            }],
            policies: vec![],
            secrets: vec![],
            defaults: Some(ManifestDefaults {
                policy: None,
                cap_grants: vec![cap_grant],
            }),
            module_bindings: IndexMap::new(),
            routing: None,
            triggers: vec![],
        };
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
        let manifest = Manifest {
            schemas: vec![NamedRef {
                name: secret_schema.schema.name.clone(),
                hash: secret_schema.hash_ref.clone(),
            }],
            modules: vec![],
            plans: vec![],
            caps: vec![NamedRef {
                name: defcap.name.clone(),
                hash: HashRef::new(defcap_hash.to_hex()).unwrap(),
            }],
            policies: vec![],
            secrets: vec![],
            defaults: Some(ManifestDefaults {
                policy: None,
                cap_grants: vec![cap_grant],
            }),
            module_bindings: IndexMap::new(),
            routing: None,
            triggers: vec![],
        };
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::InvalidSecretVersion { .. }));
    }

    #[test]
    fn rejects_duplicate_secret_decl() {
        let store = MemStore::default();
        let manifest = Manifest {
            schemas: vec![],
            modules: vec![],
            plans: vec![],
            caps: vec![],
            policies: vec![],
            secrets: vec![
                SecretDecl {
                    alias: "payments/stripe".into(),
                    version: 1,
                    binding_id: "stripe:prod".into(),
                    expected_digest: None,
                    policy: None,
                },
                SecretDecl {
                    alias: "payments/stripe".into(),
                    version: 1,
                    binding_id: "stripe:prod".into(),
                    expected_digest: None,
                    policy: None,
                },
            ],
            defaults: None,
            module_bindings: IndexMap::new(),
            routing: None,
            triggers: vec![],
        };
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::DuplicateSecret { .. }));
    }

    #[test]
    fn rejects_secret_missing_binding() {
        let store = MemStore::default();
        let manifest = Manifest {
            schemas: vec![],
            modules: vec![],
            plans: vec![],
            caps: vec![],
            policies: vec![],
            secrets: vec![SecretDecl {
                alias: "payments/stripe".into(),
                version: 1,
                binding_id: " ".into(),
                expected_digest: None,
                policy: None,
            }],
            defaults: None,
            module_bindings: IndexMap::new(),
            routing: None,
            triggers: vec![],
        };
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::SecretMissingBinding { .. }));
    }

    #[test]
    fn rejects_secret_policy_for_cap() {
        let store = MemStore::default();
        let manifest = Manifest {
            schemas: vec![],
            modules: vec![],
            plans: vec![],
            caps: vec![],
            policies: vec![],
            secrets: vec![SecretDecl {
                alias: "payments/stripe".into(),
                version: 1,
                binding_id: "stripe:prod".into(),
                expected_digest: None,
                policy: Some(SecretPolicy {
                    allowed_caps: vec!["other_cap".into()],
                    allowed_plans: vec![],
                }),
            }],
            defaults: Some(ManifestDefaults {
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
            }),
            module_bindings: IndexMap::new(),
            routing: None,
            triggers: vec![],
        };
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::SecretPolicyViolation { .. }));
    }
}
