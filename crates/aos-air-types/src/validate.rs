use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::{
    DefCap, DefEffect, DefModule, DefPolicy, DefSchema, Manifest, ModuleKind, RoutingEvent,
    SecretEntry, TypeExpr, builtins,
};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("route to keyed module '{module}' must specify key_field")]
    RoutingMissingKeyField { module: String },
    #[error("route to non-keyed module '{module}' must not specify key_field")]
    RoutingUnexpectedKeyField { module: String },
    #[error("route to module '{module}' references unknown module")]
    RoutingUnknownModule { module: String },
    #[error("route to module '{module}' uses schema '{event}' but module ABI declares '{expected}'")]
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
    #[error("capability grant '{cap}' not found")]
    CapabilityNotFound { cap: String },
    #[error("capability grant '{cap}' is duplicated")]
    DuplicateCapabilityGrant { cap: String },
    #[error("capability definition '{cap}' not found")]
    CapabilityDefinitionNotFound { cap: String },
    #[error(
        "capability '{cap}' type '{found}' does not match effect '{effect}' required type '{expected}'"
    )]
    CapabilityTypeMismatch {
        cap: String,
        effect: String,
        expected: String,
        found: String,
    },
    #[error("workflow module '{module}' must define reducer ABI")]
    WorkflowAbiMissingReducer { module: String },
    #[error("pure module '{module}' must define pure ABI")]
    PureAbiMissingPure { module: String },
    #[error("pure module '{module}' must not define reducer ABI")]
    PureAbiHasReducer { module: String },
}

pub fn validate_manifest(
    manifest: &Manifest,
    modules: &HashMap<String, DefModule>,
    schemas: &HashMap<String, DefSchema>,
    effects: &HashMap<String, DefEffect>,
    caps: &HashMap<String, DefCap>,
    policies: &HashMap<String, DefPolicy>,
) -> Result<(), ValidationError> {
    let schema_exists = |name: &str| {
        schemas.contains_key(name) || builtins::find_builtin_schema(name).is_some()
    };
    let schema_type = |name: &str| -> Option<TypeExpr> {
        schemas
            .get(name)
            .map(|schema| schema.ty.clone())
            .or_else(|| builtins::find_builtin_schema(name).map(|builtin| builtin.schema.ty.clone()))
    };

    let mut known_effect_kinds: HashSet<String> = builtins::builtin_effects()
        .iter()
        .map(|e| e.effect.kind.as_str().to_string())
        .collect();
    known_effect_kinds.extend(effects.values().map(|def| def.kind.as_str().to_string()));

    let mut effect_cap_types: HashMap<String, String> = HashMap::new();
    for builtin in builtins::builtin_effects() {
        effect_cap_types.insert(
            builtin.effect.kind.as_str().to_string(),
            builtin.effect.cap_type.as_str().to_string(),
        );
    }
    for effect in effects.values() {
        effect_cap_types.insert(
            effect.kind.as_str().to_string(),
            effect.cap_type.as_str().to_string(),
        );
    }

    let mut defcap_types: HashMap<String, String> = HashMap::new();
    for builtin in builtins::builtin_caps() {
        defcap_types.insert(
            builtin.cap.name.clone(),
            builtin.cap.cap_type.as_str().to_string(),
        );
    }
    for cap in caps.values() {
        defcap_types.insert(cap.name.clone(), cap.cap_type.as_str().to_string());
    }

    let defcap_listed = |name: &str| {
        manifest.caps.iter().any(|cap| cap.name.as_str() == name)
            || builtins::find_builtin_cap(name).is_some()
    };

    let mut grant_map: HashMap<String, String> = HashMap::new();
    if let Some(defaults) = manifest.defaults.as_ref() {
        for grant in &defaults.cap_grants {
            if grant_map.insert(grant.name.clone(), grant.cap.clone()).is_some() {
                return Err(ValidationError::DuplicateCapabilityGrant {
                    cap: grant.name.clone(),
                });
            }
            if !defcap_listed(grant.cap.as_str()) || !defcap_types.contains_key(grant.cap.as_str()) {
                return Err(ValidationError::CapabilityDefinitionNotFound {
                    cap: grant.cap.clone(),
                });
            }
        }
    }

    let grant_exists = |name: &str| grant_map.contains_key(name);
    let cap_type_for_grant = |grant_name: &str| -> Result<String, ValidationError> {
        let cap_name = grant_map
            .get(grant_name)
            .ok_or_else(|| ValidationError::CapabilityNotFound {
                cap: grant_name.to_string(),
            })?;
        if !defcap_listed(cap_name.as_str()) {
            return Err(ValidationError::CapabilityDefinitionNotFound {
                cap: cap_name.clone(),
            });
        }
        defcap_types
            .get(cap_name.as_str())
            .cloned()
            .ok_or_else(|| ValidationError::CapabilityDefinitionNotFound {
                cap: cap_name.clone(),
            })
    };

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

            let module_def = modules
                .get(module)
                .ok_or_else(|| ValidationError::RoutingUnknownModule {
                    module: module.clone(),
                })?;
            let reducer_abi = module_def
                .abi
                .reducer
                .as_ref()
                .ok_or_else(|| ValidationError::WorkflowAbiMissingReducer {
                    module: module.clone(),
                })?;

            let expected = reducer_abi.event.as_str();
            let family_schema = schema_type(expected).ok_or_else(|| ValidationError::SchemaNotFound {
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
                let event_schema = schema_type(event.as_str()).ok_or_else(|| {
                    ValidationError::SchemaNotFound {
                        schema: event.as_str().to_string(),
                    }
                })?;
                let field_ty = key_field_type(&event_schema, field, &schema_type).ok_or_else(|| {
                    ValidationError::RoutingKeyFieldMismatch {
                        module: module.clone(),
                        event: event.as_str().to_string(),
                        key_field: field.to_string(),
                        expected: key_schema_name.to_string(),
                        found: "missing".into(),
                    }
                })?;
                let matches = key_type_matches(&field_ty, &key_schema, &schema_type).unwrap_or(false);
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

    for (module_name, module) in modules {
        match module.module_kind {
            ModuleKind::Workflow => {
                if module.abi.reducer.is_none() {
                    return Err(ValidationError::WorkflowAbiMissingReducer {
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
                if module.abi.reducer.is_some() {
                    return Err(ValidationError::PureAbiHasReducer {
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

        if let Some(abi) = module.abi.reducer.as_ref() {
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
            let event_schema = schema_type(event_schema_name).ok_or_else(|| {
                ValidationError::SchemaNotFound {
                    schema: event_schema_name.to_string(),
                }
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
        for schema_ref in [effect.params_schema.as_str(), effect.receipt_schema.as_str()] {
            if !schema_exists(schema_ref) {
                return Err(ValidationError::SchemaNotFound {
                    schema: schema_ref.to_string(),
                });
            }
        }
    }

    for policy in policies.values() {
        for rule in &policy.rules {
            if let Some(kind) = rule.when.effect_kind.as_ref()
                && !known_effect_kinds.contains(kind.as_str())
            {
                return Err(ValidationError::EffectNotFound {
                    kind: kind.as_str().to_string(),
                });
            }
            if let Some(cap) = rule.when.cap_name.as_ref()
                && !grant_exists(cap.as_str())
            {
                return Err(ValidationError::CapabilityNotFound { cap: cap.clone() });
            }
        }
    }

    for binding in manifest.module_bindings.values() {
        for cap in binding.slots.values() {
            if !grant_exists(cap.as_str()) {
                return Err(ValidationError::CapabilityNotFound { cap: cap.clone() });
            }
        }
    }

    for secret in &manifest.secrets {
        let SecretEntry::Decl(secret) = secret else {
            continue;
        };
        if let Some(policy) = secret.policy.as_ref() {
            for cap in &policy.allowed_caps {
                if !grant_exists(cap.as_str()) {
                    return Err(ValidationError::CapabilityNotFound { cap: cap.clone() });
                }
            }
        }
    }

    for module in modules.values() {
        if let Some(abi) = module.abi.reducer.as_ref() {
            for effect in &abi.effects_emitted {
                let expected = effect_cap_types.get(effect.as_str()).ok_or_else(|| {
                    ValidationError::EffectNotFound {
                        kind: effect.as_str().to_string(),
                    }
                })?;
                for grant in module
                    .abi
                    .reducer
                    .as_ref()
                    .map(|r| r.cap_slots.values())
                    .into_iter()
                    .flatten()
                {
                    let _ = grant;
                }
                for binding in manifest
                    .module_bindings
                    .get(&module.name)
                    .map(|b| b.slots.values())
                    .into_iter()
                    .flatten()
                {
                    let found = cap_type_for_grant(binding.as_str())?;
                    if &found != expected {
                        return Err(ValidationError::CapabilityTypeMismatch {
                            cap: binding.clone(),
                            effect: effect.as_str().to_string(),
                            expected: expected.clone(),
                            found,
                        });
                    }
                }
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
        TypeExpr::Variant(variant) => variant.variant.values().any(|ty| {
            matches!(ty, TypeExpr::Ref(reference) if reference.reference.as_str() == event)
        }),
        _ => false,
    }
}

fn receipt_schema_allows_missing_key_field(event_schema: &str) -> bool {
    matches!(
        event_schema,
        "sys/TimerFired@1" | "sys/BlobPutResult@1" | "sys/BlobGetResult@1"
    )
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
        CapGrant, DefModule, ManifestDefaults, ModuleAbi, ModuleBinding, NamedRef, ReducerAbi,
        Routing, SchemaRef, TypePrimitive, TypePrimitiveText,
    };
    use indexmap::IndexMap;

    fn text_type() -> TypeExpr {
        TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
            text: crate::EmptyObject::default(),
        }))
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
            caps: vec![named_ref("com.acme/http@1")],
            policies: Vec::new(),
            secrets: Vec::new(),
            defaults: Some(ManifestDefaults {
                policy: None,
                cap_grants: vec![CapGrant {
                    name: "http_cap".into(),
                    cap: "com.acme/http@1".into(),
                    params: crate::ValueLiteral::Record(crate::ValueRecord {
                        record: IndexMap::new(),
                    }),
                    expiry_ns: None,
                }],
            }),
            module_bindings: IndexMap::new(),
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
                reducer: Some(ReducerAbi {
                    state: SchemaRef::new("com.acme/State@1").unwrap(),
                    event: SchemaRef::new("com.acme/Event@1").unwrap(),
                    context: None,
                    annotations: None,
                    effects_emitted: Vec::new(),
                    cap_slots: IndexMap::new(),
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
                    ty: text_type(),
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
        let effects = HashMap::new();
        let caps = HashMap::from([(
            String::from("com.acme/http@1"),
            DefCap {
                name: "com.acme/http@1".into(),
                cap_type: crate::CapType::http_out(),
                schema: text_type(),
                enforcer: crate::CapEnforcer {
                    module: "sys/CapAllowAll@1".into(),
                },
            },
        )]);
        let policies = HashMap::new();

        assert!(validate_manifest(&manifest, &modules, &schemas, &effects, &caps, &policies)
            .is_ok());
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
                    ty: text_type(),
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

        let err =
            validate_manifest(&manifest, &modules, &schemas, &HashMap::new(), &HashMap::new(), &HashMap::new())
                .unwrap_err();
        assert!(matches!(
            err,
            ValidationError::RoutingUnknownModule { module } if module == "com.acme/missing@1"
        ));
    }

    #[test]
    fn validate_manifest_rejects_missing_binding_grant() {
        let mut manifest = base_manifest();
        manifest.module_bindings.insert(
            "com.acme/workflow@1".into(),
            ModuleBinding {
                slots: IndexMap::from([(String::from("http"), String::from("missing_grant"))]),
            },
        );

        let modules = HashMap::from([(String::from("com.acme/workflow@1"), base_module())]);
        let schemas = HashMap::from([
            (
                String::from("com.acme/Event@1"),
                DefSchema {
                    name: "com.acme/Event@1".into(),
                    ty: text_type(),
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

        let err =
            validate_manifest(&manifest, &modules, &schemas, &HashMap::new(), &HashMap::new(), &HashMap::new())
                .unwrap_err();
        assert!(matches!(
            err,
            ValidationError::CapabilityNotFound { cap } if cap == "missing_grant"
        ));
    }
}
