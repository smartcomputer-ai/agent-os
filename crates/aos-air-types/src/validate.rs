use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::{
    DefModule, DefOp, DefSchema, DefSecret, Manifest, ModuleRuntime, OpKind, RoutingEvent,
    TypeExpr, builtins,
};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("manifest {kind} ref '{name}' does not resolve")]
    ManifestRefNotFound { kind: &'static str, name: String },
    #[error("op '{op}' implementation references inactive module '{module}'")]
    OpUnknownModule { op: String, module: String },
    #[error("workflow op '{op}' must define workflow and not effect")]
    WorkflowOpShape { op: String },
    #[error("effect op '{op}' must define effect and not workflow")]
    EffectOpShape { op: String },
    #[error("schema '{schema}' not found")]
    SchemaNotFound { schema: String },
    #[error("effect op '{op}' not found or not active")]
    EffectOpNotFound { op: String },
    #[error("workflow op '{op}' not found or not active")]
    WorkflowOpNotFound { op: String },
    #[error("route to keyed workflow op '{op}' must specify key_field")]
    RoutingMissingKeyField { op: String },
    #[error("route to non-keyed workflow op '{op}' must not specify key_field")]
    RoutingUnexpectedKeyField { op: String },
    #[error(
        "route to workflow op '{op}' uses schema '{event}' but workflow event schema is '{expected}'"
    )]
    RoutingSchemaMismatch {
        op: String,
        event: String,
        expected: String,
    },
    #[error(
        "route to workflow op '{op}' uses key_field '{key_field}' with schema '{event}', but key schema '{expected}' does not match '{found}'"
    )]
    RoutingKeyFieldMismatch {
        op: String,
        event: String,
        key_field: String,
        expected: String,
        found: String,
    },
    #[error("workflow op '{op}' event family schema '{event_schema}' is invalid: {reason}")]
    WorkflowEventFamilyInvalid {
        op: String,
        event_schema: String,
        reason: String,
    },
    #[error("module '{module}' runtime '{runtime}' does not support {op_kind} op '{op}'")]
    UnsupportedRuntimeForOp {
        module: String,
        runtime: &'static str,
        op_kind: &'static str,
        op: String,
    },
}

pub fn validate_manifest(
    manifest: &Manifest,
    modules: &HashMap<String, DefModule>,
    schemas: &HashMap<String, DefSchema>,
    ops: &HashMap<String, DefOp>,
    secrets: &HashMap<String, DefSecret>,
) -> Result<(), ValidationError> {
    let active_schemas: HashSet<String> = manifest.schemas.iter().map(|r| r.name.clone()).collect();
    let active_modules: HashSet<String> = manifest.modules.iter().map(|r| r.name.clone()).collect();
    let active_ops: HashSet<String> = manifest.ops.iter().map(|r| r.name.clone()).collect();
    let active_secrets: HashSet<String> = manifest.secrets.iter().map(|r| r.name.clone()).collect();

    for reference in &manifest.schemas {
        if !schemas.contains_key(&reference.name)
            && builtins::find_builtin_schema(reference.name.as_str()).is_none()
        {
            return Err(ValidationError::ManifestRefNotFound {
                kind: "schema",
                name: reference.name.clone(),
            });
        }
    }
    for reference in &manifest.modules {
        if !modules.contains_key(&reference.name)
            && builtins::find_builtin_module(reference.name.as_str()).is_none()
        {
            return Err(ValidationError::ManifestRefNotFound {
                kind: "module",
                name: reference.name.clone(),
            });
        }
    }
    for reference in &manifest.ops {
        if !ops.contains_key(&reference.name)
            && builtins::find_builtin_op(reference.name.as_str()).is_none()
        {
            return Err(ValidationError::ManifestRefNotFound {
                kind: "op",
                name: reference.name.clone(),
            });
        }
    }
    for reference in &manifest.secrets {
        if !secrets.contains_key(&reference.name) {
            return Err(ValidationError::ManifestRefNotFound {
                kind: "secret",
                name: reference.name.clone(),
            });
        }
    }

    let schema_exists =
        |name: &str| active_schemas.contains(name) || builtins::find_builtin_schema(name).is_some();
    let schema_type = |name: &str| -> Option<TypeExpr> {
        schemas
            .get(name)
            .map(|schema| schema.ty.clone())
            .or_else(|| {
                builtins::find_builtin_schema(name).map(|builtin| builtin.schema.ty.clone())
            })
    };
    let op_lookup = |name: &str| -> Option<DefOp> {
        ops.get(name)
            .cloned()
            .or_else(|| builtins::find_builtin_op(name).map(|builtin| builtin.op.clone()))
    };
    let module_lookup = |name: &str| -> Option<DefModule> {
        modules
            .get(name)
            .cloned()
            .or_else(|| builtins::find_builtin_module(name).map(|builtin| builtin.module.clone()))
    };

    for op_name in &active_ops {
        let op = op_lookup(op_name).ok_or_else(|| ValidationError::ManifestRefNotFound {
            kind: "op",
            name: op_name.clone(),
        })?;
        if !active_modules.contains(&op.implementation.module)
            && builtins::find_builtin_module(op.implementation.module.as_str()).is_none()
        {
            return Err(ValidationError::OpUnknownModule {
                op: op.name,
                module: op.implementation.module,
            });
        }

        let module = module_lookup(&op.implementation.module).expect("module checked above");
        validate_runtime_support(&op, &module)?;

        match op.op_kind {
            OpKind::Workflow => {
                let Some(workflow) = op.workflow.as_ref() else {
                    return Err(ValidationError::WorkflowOpShape { op: op.name });
                };
                if op.effect.is_some() {
                    return Err(ValidationError::WorkflowOpShape { op: op.name });
                }
                for schema_ref in [
                    Some(workflow.state.as_str()),
                    Some(workflow.event.as_str()),
                    workflow.context.as_ref().map(|s| s.as_str()),
                    workflow.annotations.as_ref().map(|s| s.as_str()),
                    workflow.key_schema.as_ref().map(|s| s.as_str()),
                ]
                .iter()
                .flatten()
                {
                    if !schema_exists(schema_ref) {
                        return Err(ValidationError::SchemaNotFound {
                            schema: schema_ref.to_string(),
                        });
                    }
                }
                let event_schema_name = workflow.event.as_str();
                let event_schema = schema_type(event_schema_name).ok_or_else(|| {
                    ValidationError::SchemaNotFound {
                        schema: event_schema_name.to_string(),
                    }
                })?;
                validate_event_family(op.name.as_str(), event_schema_name, &event_schema)?;
                for effect_op in &workflow.effects_emitted {
                    let Some(effect_def) = op_lookup(effect_op) else {
                        return Err(ValidationError::EffectOpNotFound {
                            op: effect_op.clone(),
                        });
                    };
                    if !active_ops.contains(effect_op) || effect_def.op_kind != OpKind::Effect {
                        return Err(ValidationError::EffectOpNotFound {
                            op: effect_op.clone(),
                        });
                    }
                }
            }
            OpKind::Effect => {
                let Some(effect) = op.effect.as_ref() else {
                    return Err(ValidationError::EffectOpShape { op: op.name });
                };
                if op.workflow.is_some() {
                    return Err(ValidationError::EffectOpShape { op: op.name });
                }
                for schema_ref in [effect.params.as_str(), effect.receipt.as_str()] {
                    if !schema_exists(schema_ref) {
                        return Err(ValidationError::SchemaNotFound {
                            schema: schema_ref.to_string(),
                        });
                    }
                }
            }
        }
    }

    if let Some(routing) = manifest.routing.as_ref() {
        for RoutingEvent {
            event,
            op,
            key_field,
        } in &routing.subscriptions
        {
            if !schema_exists(event.as_str()) {
                return Err(ValidationError::SchemaNotFound {
                    schema: event.as_str().to_string(),
                });
            }
            let Some(op_def) = op_lookup(op) else {
                return Err(ValidationError::WorkflowOpNotFound { op: op.clone() });
            };
            if !active_ops.contains(op) || op_def.op_kind != OpKind::Workflow {
                return Err(ValidationError::WorkflowOpNotFound { op: op.clone() });
            }
            let workflow = op_def
                .workflow
                .as_ref()
                .ok_or_else(|| ValidationError::WorkflowOpShape { op: op.clone() })?;
            let expected = workflow.event.as_str();
            let family_schema =
                schema_type(expected).ok_or_else(|| ValidationError::SchemaNotFound {
                    schema: expected.to_string(),
                })?;
            if !event_in_family(event.as_str(), expected, &family_schema) {
                return Err(ValidationError::RoutingSchemaMismatch {
                    op: op.clone(),
                    event: event.as_str().to_string(),
                    expected: expected.to_string(),
                });
            }

            let keyed = workflow.key_schema.is_some();
            match (keyed, key_field.is_some()) {
                (true, false) => {
                    return Err(ValidationError::RoutingMissingKeyField { op: op.clone() });
                }
                (false, true) => {
                    return Err(ValidationError::RoutingUnexpectedKeyField { op: op.clone() });
                }
                _ => {}
            }

            if let (Some(key_schema_ref), Some(field)) =
                (workflow.key_schema.as_ref(), key_field.as_ref())
            {
                let key_schema_name = key_schema_ref.as_str();
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
                            op: op.clone(),
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
                        op: op.clone(),
                        event: event.as_str().to_string(),
                        key_field: field.to_string(),
                        expected: key_schema_name.to_string(),
                        found: type_name(&field_ty),
                    });
                }
            }
        }
    }

    for secret in &active_secrets {
        if !secrets.contains_key(secret) {
            return Err(ValidationError::ManifestRefNotFound {
                kind: "secret",
                name: secret.clone(),
            });
        }
    }

    Ok(())
}

fn validate_runtime_support(op: &DefOp, module: &DefModule) -> Result<(), ValidationError> {
    match (&module.runtime, op.op_kind) {
        (ModuleRuntime::Builtin {}, OpKind::Workflow) => {
            Err(ValidationError::UnsupportedRuntimeForOp {
                module: module.name.clone(),
                runtime: "builtin",
                op_kind: "workflow",
                op: op.name.clone(),
            })
        }
        _ => Ok(()),
    }
}

fn validate_event_family(
    op_name: &str,
    event_schema_name: &str,
    event_schema: &TypeExpr,
) -> Result<(), ValidationError> {
    match event_schema {
        TypeExpr::Ref(_) => Ok(()),
        TypeExpr::Variant(variant) => {
            let mut seen = HashSet::new();
            for ty in variant.variant.values() {
                let TypeExpr::Ref(reference) = ty else {
                    return Err(ValidationError::WorkflowEventFamilyInvalid {
                        op: op_name.to_string(),
                        event_schema: event_schema_name.to_string(),
                        reason: "variant arm is not a ref".into(),
                    });
                };
                let name = reference.reference.as_str().to_string();
                if !seen.insert(name) {
                    return Err(ValidationError::WorkflowEventFamilyInvalid {
                        op: op_name.to_string(),
                        event_schema: event_schema_name.to_string(),
                        reason: "duplicate event schema in variant".into(),
                    });
                }
            }
            Ok(())
        }
        TypeExpr::Record(_) => Ok(()),
        _ => Err(ValidationError::WorkflowEventFamilyInvalid {
            op: op_name.to_string(),
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

    let mut current = resolve_type(event_schema, schema_type)?;
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
