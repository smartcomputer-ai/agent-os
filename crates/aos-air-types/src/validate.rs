use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::{
    DefEffect, DefModule, DefSchema, Manifest, ModuleKind, RoutingEvent, TypeExpr, builtins,
};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("route to keyed module '{module}' must specify key_field")]
    RoutingMissingKeyField { module: String },
    #[error("route to non-keyed module '{module}' must not specify key_field")]
    RoutingUnexpectedKeyField { module: String },
    #[error("route to module '{module}' references unknown module")]
    RoutingUnknownModule { module: String },
    #[error(
        "route to module '{module}' uses schema '{event}' but module ABI declares '{expected}'"
    )]
    RoutingSchemaMismatch {
        module: String,
        event: String,
        expected: String,
    },
    #[error(
        "route to module '{module}' uses key_field '{key_field}' with schema '{event}', but key schema '{expected}' does not match '{found}'"
    )]
    RoutingKeyFieldMismatch {
        module: String,
        event: String,
        key_field: String,
        expected: String,
        found: String,
    },
    #[error("module '{module}' event family schema '{event_schema}' is invalid: {reason}")]
    ModuleEventFamilyInvalid {
        module: String,
        event_schema: String,
        reason: String,
    },
    #[error("schema '{schema}' not found")]
    SchemaNotFound { schema: String },
    #[error("effect kind '{kind}' not found in catalog or built-ins")]
    EffectNotFound { kind: String },
    #[error("effect binding kind '{kind}' is not declared in manifest.effects")]
    EffectBindingKindNotDeclared { kind: String },
    #[error("effect binding kind '{kind}' is duplicated")]
    EffectBindingDuplicateKind { kind: String },
    #[error("effect binding kind '{kind}' is internal and cannot be bound")]
    EffectBindingInternalKind { kind: String },
    #[error("workflow module '{module}' must define workflow ABI")]
    WorkflowAbiMissingWorkflow { module: String },
    #[error("pure module '{module}' must define pure ABI")]
    PureAbiMissingPure { module: String },
    #[error("pure module '{module}' must not define workflow ABI")]
    PureAbiHasWorkflow { module: String },
}

pub fn validate_manifest(
    manifest: &Manifest,
    modules: &HashMap<String, DefModule>,
    schemas: &HashMap<String, DefSchema>,
    effects: &HashMap<String, DefEffect>,
) -> Result<(), ValidationError> {
    let schema_exists =
        |name: &str| schemas.contains_key(name) || builtins::find_builtin_schema(name).is_some();
    let schema_type = |name: &str| -> Option<TypeExpr> {
        schemas
            .get(name)
            .map(|schema| schema.ty.clone())
            .or_else(|| {
                builtins::find_builtin_schema(name).map(|builtin| builtin.schema.ty.clone())
            })
    };

    let mut known_effect_kinds: HashSet<String> = builtins::builtin_effects()
        .iter()
        .map(|e| e.effect.kind.as_str().to_string())
        .collect();
    known_effect_kinds.extend(effects.values().map(|def| def.kind.as_str().to_string()));
    let declared_effect_kinds: HashSet<String> = effects
        .values()
        .map(|def| def.kind.as_str().to_string())
        .collect();

    if let Some(routing) = manifest.routing.as_ref() {
        for RoutingEvent {
            event,
            module,
            key_field,
        } in &routing.subscriptions
        {
            if !schema_exists(event.as_str()) {
                return Err(ValidationError::SchemaNotFound {
                    schema: event.as_str().to_string(),
                });
            }

            let module_def =
                modules
                    .get(module)
                    .ok_or_else(|| ValidationError::RoutingUnknownModule {
                        module: module.clone(),
                    })?;
            let workflow_abi = module_def.abi.workflow.as_ref().ok_or_else(|| {
                ValidationError::WorkflowAbiMissingWorkflow {
                    module: module.clone(),
                }
            })?;

            let expected = workflow_abi.event.as_str();
            let family_schema =
                schema_type(expected).ok_or_else(|| ValidationError::SchemaNotFound {
                    schema: expected.to_string(),
                })?;
            if !event_in_family(event.as_str(), expected, &family_schema) {
                return Err(ValidationError::RoutingSchemaMismatch {
                    module: module.clone(),
                    event: event.as_str().to_string(),
                    expected: expected.to_string(),
                });
            }

            let keyed = module_def.key_schema.is_some();
            match (keyed, key_field.is_some()) {
                (true, false) => {
                    if !receipt_schema_allows_missing_key_field(event.as_str()) {
                        return Err(ValidationError::RoutingMissingKeyField {
                            module: module.clone(),
                        });
                    }
                }
                (false, true) => {
                    return Err(ValidationError::RoutingUnexpectedKeyField {
                        module: module.clone(),
                    });
                }
                _ => {}
            }

            if let (true, Some(field)) = (keyed, key_field.as_ref()) {
                let key_schema_name = module_def
                    .key_schema
                    .as_ref()
                    .expect("keyed modules have key_schema")
                    .as_str();
                let key_schema = schema_type(key_schema_name).ok_or_else(|| {
                    ValidationError::SchemaNotFound {
                        schema: key_schema_name.to_string(),
                    }
                })?;
                let event_schema =
                    schema_type(event.as_str()).ok_or_else(|| ValidationError::SchemaNotFound {
                        schema: event.as_str().to_string(),
                    })?;
                let field_ty =
                    key_field_type(&event_schema, field, &schema_type).ok_or_else(|| {
                        ValidationError::RoutingKeyFieldMismatch {
                            module: module.clone(),
                            event: event.as_str().to_string(),
                            key_field: field.to_string(),
                            expected: key_schema_name.to_string(),
                            found: "missing".into(),
                        }
                    })?;
                let matches =
                    key_type_matches(&field_ty, &key_schema, &schema_type).unwrap_or(false);
                if !matches {
                    return Err(ValidationError::RoutingKeyFieldMismatch {
                        module: module.clone(),
                        event: event.as_str().to_string(),
                        key_field: field.to_string(),
                        expected: key_schema_name.to_string(),
                        found: type_name(&field_ty),
                    });
                }
            }
        }
    }

    let mut bound_kinds = HashSet::new();
    for binding in &manifest.effect_bindings {
        let kind = binding.kind.as_str();
        if is_internal_effect_kind(kind) {
            return Err(ValidationError::EffectBindingInternalKind {
                kind: kind.to_string(),
            });
        }
        if !declared_effect_kinds.contains(kind) {
            return Err(ValidationError::EffectBindingKindNotDeclared {
                kind: kind.to_string(),
            });
        }
        if !bound_kinds.insert(kind.to_string()) {
            return Err(ValidationError::EffectBindingDuplicateKind {
                kind: kind.to_string(),
            });
        }
    }

    for (module_name, module) in modules {
        match module.module_kind {
            ModuleKind::Workflow => {
                if module.abi.workflow.is_none() {
                    return Err(ValidationError::WorkflowAbiMissingWorkflow {
                        module: module_name.clone(),
                    });
                }
            }
            ModuleKind::Pure => {
                if module.abi.pure.is_none() {
                    return Err(ValidationError::PureAbiMissingPure {
                        module: module_name.clone(),
                    });
                }
                if module.abi.workflow.is_some() {
                    return Err(ValidationError::PureAbiHasWorkflow {
                        module: module_name.clone(),
                    });
                }
            }
        }

        if let Some(key) = module.key_schema.as_ref() {
            if !schema_exists(key.as_str()) {
                return Err(ValidationError::SchemaNotFound {
                    schema: key.as_str().to_string(),
                });
            }
        }

        if let Some(abi) = module.abi.workflow.as_ref() {
            for schema_ref in [
                Some(abi.state.as_str()),
                Some(abi.event.as_str()),
                abi.context.as_ref().map(|s| s.as_str()),
                abi.annotations.as_ref().map(|s| s.as_str()),
            ]
            .iter()
            .flatten()
            .filter(|s| !s.is_empty())
            {
                if !schema_exists(schema_ref) {
                    return Err(ValidationError::SchemaNotFound {
                        schema: schema_ref.to_string(),
                    });
                }
            }

            let event_schema_name = abi.event.as_str();
            let event_schema =
                schema_type(event_schema_name).ok_or_else(|| ValidationError::SchemaNotFound {
                    schema: event_schema_name.to_string(),
                })?;
            validate_event_family(module_name, event_schema_name, &event_schema)?;

            for effect in &abi.effects_emitted {
                if !known_effect_kinds.contains(effect.as_str()) {
                    return Err(ValidationError::EffectNotFound {
                        kind: effect.as_str().to_string(),
                    });
                }
            }
        }

        if let Some(abi) = module.abi.pure.as_ref() {
            for schema_ref in [abi.input.as_str(), abi.output.as_str()]
                .into_iter()
                .chain(abi.context.as_ref().map(|s| s.as_str()))
            {
                if !schema_exists(schema_ref) {
                    return Err(ValidationError::SchemaNotFound {
                        schema: schema_ref.to_string(),
                    });
                }
            }
        }
    }

    for effect in effects.values() {
        for schema_ref in [
            effect.params_schema.as_str(),
            effect.receipt_schema.as_str(),
        ] {
            if !schema_exists(schema_ref) {
                return Err(ValidationError::SchemaNotFound {
                    schema: schema_ref.to_string(),
                });
            }
        }
    }

    Ok(())
}

fn validate_event_family(
    module_name: &str,
    event_schema_name: &str,
    event_schema: &TypeExpr,
) -> Result<(), ValidationError> {
    match event_schema {
        TypeExpr::Ref(_) => Ok(()),
        TypeExpr::Variant(variant) => {
            let mut seen = HashSet::new();
            for ty in variant.variant.values() {
                let TypeExpr::Ref(reference) = ty else {
                    return Err(ValidationError::ModuleEventFamilyInvalid {
                        module: module_name.to_string(),
                        event_schema: event_schema_name.to_string(),
                        reason: "variant arm is not a ref".into(),
                    });
                };
                let name = reference.reference.as_str().to_string();
                if !seen.insert(name) {
                    return Err(ValidationError::ModuleEventFamilyInvalid {
                        module: module_name.to_string(),
                        event_schema: event_schema_name.to_string(),
                        reason: "duplicate event schema in variant".into(),
                    });
                }
            }
            Ok(())
        }
        TypeExpr::Record(_) => Ok(()),
        _ => Err(ValidationError::ModuleEventFamilyInvalid {
            module: module_name.to_string(),
            event_schema: event_schema_name.to_string(),
            reason: "event family must be a ref, variant of refs, or record".into(),
        }),
    }
}

fn event_in_family(event: &str, family_name: &str, family_schema: &TypeExpr) -> bool {
    if event == family_name {
        return true;
    }
    match family_schema {
        TypeExpr::Ref(reference) => reference.reference.as_str() == event,
        TypeExpr::Variant(variant) => variant.variant.values().any(
            |ty| matches!(ty, TypeExpr::Ref(reference) if reference.reference.as_str() == event),
        ),
        _ => false,
    }
}

fn receipt_schema_allows_missing_key_field(event_schema: &str) -> bool {
    matches!(
        event_schema,
        "sys/TimerFired@1" | "sys/BlobPutResult@1" | "sys/BlobGetResult@1"
    )
}

fn is_internal_effect_kind(kind: &str) -> bool {
    kind.starts_with("workspace.")
        || kind.starts_with("introspect.")
        || kind.starts_with("governance.")
}

fn resolve_type(
    ty: &TypeExpr,
    schema_type: &impl Fn(&str) -> Option<TypeExpr>,
) -> Option<TypeExpr> {
    match ty {
        TypeExpr::Ref(reference) => schema_type(reference.reference.as_str()),
        _ => Some(ty.clone()),
    }
}

fn type_eq(left: &TypeExpr, right: &TypeExpr) -> bool {
    match (serde_json::to_value(left), serde_json::to_value(right)) {
        (Ok(l), Ok(r)) => l == r,
        _ => false,
    }
}

fn type_name(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Ref(reference) => reference.reference.as_str().to_string(),
        _ => format!("{ty:?}"),
    }
}

fn key_field_type(
    event_schema: &TypeExpr,
    key_field: &str,
    schema_type: &impl Fn(&str) -> Option<TypeExpr>,
) -> Option<TypeExpr> {
    let segments: Vec<&str> = key_field.split('.').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return None;
    }

    let resolved = resolve_type(event_schema, schema_type)?;
    if let TypeExpr::Variant(variant) = &resolved {
        if segments[0] == "$value" {
            let remaining = &segments[1..];
            if remaining.is_empty() {
                return None;
            }
            let mut found: Option<TypeExpr> = None;
            for ty in variant.variant.values() {
                if let TypeExpr::Ref(reference) = ty
                    && receipt_schema_allows_missing_key_field(reference.reference.as_str())
                {
                    continue;
                }
                let resolved_arm = resolve_type(ty, schema_type)?;
                let mut current = resolved_arm;
                for seg in remaining {
                    current = match current {
                        TypeExpr::Record(record) => {
                            let field_ty = record.record.get(*seg)?;
                            resolve_type(field_ty, schema_type)?
                        }
                        _ => return None,
                    };
                }
                if let Some(existing) = &found {
                    let resolved_existing = resolve_type(existing, schema_type)?;
                    let resolved_current = resolve_type(&current, schema_type)?;
                    if !type_eq(&resolved_existing, &resolved_current) {
                        return None;
                    }
                } else {
                    found = Some(current);
                }
            }
            return found;
        }
        if segments[0] == "$tag" {
            return None;
        }
        return None;
    }

    let mut current = resolved;
    for seg in segments {
        current = match current {
            TypeExpr::Record(record) => {
                let field_ty = record.record.get(seg)?;
                resolve_type(field_ty, schema_type)?
            }
            _ => return None,
        };
    }
    Some(current)
}

fn key_type_matches(
    field_ty: &TypeExpr,
    key_schema: &TypeExpr,
    schema_type: &impl Fn(&str) -> Option<TypeExpr>,
) -> Option<bool> {
    let resolved_field = resolve_type(field_ty, schema_type)?;
    let resolved_key = resolve_type(key_schema, schema_type)?;
    if type_eq(&resolved_field, &resolved_key) {
        return Some(true);
    }
    if let (TypeExpr::Ref(field_ref), TypeExpr::Ref(key_ref)) = (field_ty, key_schema) {
        return Some(field_ref.reference == key_ref.reference);
    }
    Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DefModule, EffectBinding, ModuleAbi, NamedRef, Routing, SchemaRef, TypePrimitive,
        TypePrimitiveText, TypeRecord, WorkflowAbi,
    };
    use indexmap::IndexMap;

    fn text_type() -> TypeExpr {
        TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
            text: crate::EmptyObject::default(),
        }))
    }

    fn record_type() -> TypeExpr {
        TypeExpr::Record(TypeRecord {
            record: IndexMap::new(),
        })
    }

    fn named_ref(name: &str) -> NamedRef {
        NamedRef {
            name: name.to_string(),
            hash: crate::HashRef::new(format!("sha256:{}", "a".repeat(64))).unwrap(),
        }
    }

    fn base_manifest() -> Manifest {
        Manifest {
            air_version: "1".into(),
            schemas: vec![named_ref("com.acme/Event@1"), named_ref("com.acme/State@1")],
            modules: vec![named_ref("com.acme/workflow@1")],
            effects: Vec::new(),
            effect_bindings: Vec::new(),
            secrets: Vec::new(),
            routing: None,
        }
    }

    fn base_module() -> DefModule {
        DefModule {
            name: "com.acme/workflow@1".into(),
            module_kind: ModuleKind::Workflow,
            wasm_hash: crate::HashRef::new(format!("sha256:{}", "b".repeat(64))).unwrap(),
            key_schema: None,
            abi: ModuleAbi {
                workflow: Some(WorkflowAbi {
                    state: SchemaRef::new("com.acme/State@1").unwrap(),
                    event: SchemaRef::new("com.acme/Event@1").unwrap(),
                    context: None,
                    annotations: None,
                    effects_emitted: Vec::new(),
                }),
                pure: None,
            },
        }
    }

    #[test]
    fn validate_manifest_accepts_minimal_workflow_manifest() {
        let manifest = base_manifest();
        let modules = HashMap::from([(String::from("com.acme/workflow@1"), base_module())]);
        let schemas = HashMap::from([
            (
                String::from("com.acme/Event@1"),
                DefSchema {
                    name: "com.acme/Event@1".into(),
                    ty: record_type(),
                },
            ),
            (
                String::from("com.acme/State@1"),
                DefSchema {
                    name: "com.acme/State@1".into(),
                    ty: text_type(),
                },
            ),
        ]);
        assert!(validate_manifest(&manifest, &modules, &schemas, &HashMap::new()).is_ok());
    }

    #[test]
    fn validate_manifest_rejects_unknown_routing_module() {
        let mut manifest = base_manifest();
        manifest.routing = Some(Routing {
            subscriptions: vec![RoutingEvent {
                event: SchemaRef::new("com.acme/Event@1").unwrap(),
                module: "com.acme/missing@1".into(),
                key_field: None,
            }],
            inboxes: Vec::new(),
        });

        let modules = HashMap::from([(String::from("com.acme/workflow@1"), base_module())]);
        let schemas = HashMap::from([
            (
                String::from("com.acme/Event@1"),
                DefSchema {
                    name: "com.acme/Event@1".into(),
                    ty: record_type(),
                },
            ),
            (
                String::from("com.acme/State@1"),
                DefSchema {
                    name: "com.acme/State@1".into(),
                    ty: text_type(),
                },
            ),
        ]);

        let err = validate_manifest(&manifest, &modules, &schemas, &HashMap::new()).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::RoutingUnknownModule { module } if module == "com.acme/missing@1"
        ));
    }

    #[test]
    fn validate_manifest_rejects_effect_binding_kind_not_declared() {
        let mut manifest = base_manifest();
        manifest.effect_bindings.push(EffectBinding {
            kind: crate::EffectKind::new("http.request"),
            adapter_id: "http.default".into(),
        });

        let modules = HashMap::from([(String::from("com.acme/workflow@1"), base_module())]);
        let schemas = HashMap::from([
            (
                String::from("com.acme/Event@1"),
                DefSchema {
                    name: "com.acme/Event@1".into(),
                    ty: record_type(),
                },
            ),
            (
                String::from("com.acme/State@1"),
                DefSchema {
                    name: "com.acme/State@1".into(),
                    ty: text_type(),
                },
            ),
        ]);

        let err = validate_manifest(&manifest, &modules, &schemas, &HashMap::new()).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::EffectBindingKindNotDeclared { kind } if kind == "http.request"
        ));
    }

    #[test]
    fn validate_manifest_rejects_duplicate_effect_binding_kind() {
        let mut manifest = base_manifest();
        manifest.effect_bindings = vec![
            EffectBinding {
                kind: crate::EffectKind::new("http.request"),
                adapter_id: "http.default".into(),
            },
            EffectBinding {
                kind: crate::EffectKind::new("http.request"),
                adapter_id: "http.alt".into(),
            },
        ];

        let modules = HashMap::from([(String::from("com.acme/workflow@1"), base_module())]);
        let schemas = HashMap::from([
            (
                String::from("com.acme/Event@1"),
                DefSchema {
                    name: "com.acme/Event@1".into(),
                    ty: record_type(),
                },
            ),
            (
                String::from("com.acme/State@1"),
                DefSchema {
                    name: "com.acme/State@1".into(),
                    ty: text_type(),
                },
            ),
            (
                String::from("com.acme/HttpParams@1"),
                DefSchema {
                    name: "com.acme/HttpParams@1".into(),
                    ty: record_type(),
                },
            ),
            (
                String::from("com.acme/HttpReceipt@1"),
                DefSchema {
                    name: "com.acme/HttpReceipt@1".into(),
                    ty: record_type(),
                },
            ),
        ]);
        let effects = HashMap::from([(
            String::from("com.acme/http.request@1"),
            DefEffect {
                name: "com.acme/http.request@1".into(),
                kind: crate::EffectKind::new("http.request"),
                params_schema: SchemaRef::new("com.acme/HttpParams@1").unwrap(),
                receipt_schema: SchemaRef::new("com.acme/HttpReceipt@1").unwrap(),
                origin_scope: crate::OriginScope::Both,
            },
        )]);

        let err = validate_manifest(&manifest, &modules, &schemas, &effects).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::EffectBindingDuplicateKind { kind } if kind == "http.request"
        ));
    }

    #[test]
    fn validate_manifest_rejects_internal_effect_binding_kind() {
        let mut manifest = base_manifest();
        manifest.effect_bindings.push(EffectBinding {
            kind: crate::EffectKind::new("workspace.read_bytes"),
            adapter_id: "workspace.default".into(),
        });

        let modules = HashMap::from([(String::from("com.acme/workflow@1"), base_module())]);
        let schemas = HashMap::from([
            (
                String::from("com.acme/Event@1"),
                DefSchema {
                    name: "com.acme/Event@1".into(),
                    ty: record_type(),
                },
            ),
            (
                String::from("com.acme/State@1"),
                DefSchema {
                    name: "com.acme/State@1".into(),
                    ty: text_type(),
                },
            ),
            (
                String::from("com.acme/WorkspaceParams@1"),
                DefSchema {
                    name: "com.acme/WorkspaceParams@1".into(),
                    ty: record_type(),
                },
            ),
            (
                String::from("com.acme/WorkspaceReceipt@1"),
                DefSchema {
                    name: "com.acme/WorkspaceReceipt@1".into(),
                    ty: record_type(),
                },
            ),
        ]);
        let effects = HashMap::from([(
            String::from("com.acme/workspace.read_bytes@1"),
            DefEffect {
                name: "com.acme/workspace.read_bytes@1".into(),
                kind: crate::EffectKind::new("workspace.read_bytes"),
                params_schema: SchemaRef::new("com.acme/WorkspaceParams@1").unwrap(),
                receipt_schema: SchemaRef::new("com.acme/WorkspaceReceipt@1").unwrap(),
                origin_scope: crate::OriginScope::Both,
            },
        )]);

        let err = validate_manifest(&manifest, &modules, &schemas, &effects).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::EffectBindingInternalKind { kind } if kind == "workspace.read_bytes"
        ));
    }
}
