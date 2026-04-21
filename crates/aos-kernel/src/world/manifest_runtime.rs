use std::collections::HashMap;
use std::sync::Arc;

use crate::Store;
use aos_air_types::{
    AirNode, DefModule, HashRef, Manifest, Name, NamedRef, TypeExpr, TypePrimitive, builtins,
    catalog::EffectCatalog, schema_index::SchemaIndex,
};
use aos_cbor::Hash;

use crate::error::KernelError;
use crate::manifest::LoadedManifest;

use super::{EventWrap, RouteBinding, WorkflowSchema};

pub(super) struct RuntimeAssembly {
    pub schema_index: Arc<SchemaIndex>,
    pub workflow_schemas: Arc<HashMap<Name, WorkflowSchema>>,
    pub router: HashMap<String, Vec<RouteBinding>>,
    pub effect_catalog: Arc<EffectCatalog>,
}

pub(super) fn assemble_runtime<S: Store>(
    store: &S,
    loaded: &LoadedManifest,
) -> Result<RuntimeAssembly, KernelError> {
    let schema_index = Arc::new(build_schema_index_from_loaded(store, loaded)?);
    let workflow_schemas = Arc::new(build_workflow_schemas(
        &loaded.modules,
        schema_index.as_ref(),
    )?);
    let router = build_router(&loaded.manifest, workflow_schemas.as_ref())?;
    let effect_catalog = Arc::new(loaded.effect_catalog.clone());

    Ok(RuntimeAssembly {
        schema_index,
        workflow_schemas,
        router,
        effect_catalog,
    })
}

pub(super) fn build_schema_index_from_loaded<S: Store>(
    store: &S,
    loaded: &LoadedManifest,
) -> Result<SchemaIndex, KernelError> {
    let mut schema_map = HashMap::new();
    for builtin in builtins::builtin_schemas() {
        schema_map.insert(builtin.schema.name.clone(), builtin.schema.ty.clone());
    }
    for (name, schema) in &loaded.schemas {
        schema_map.insert(name.clone(), schema.ty.clone());
    }
    extend_schema_map_from_store(store, &loaded.manifest.schemas, &mut schema_map)?;
    Ok(SchemaIndex::new(schema_map))
}

fn build_workflow_schemas(
    modules: &HashMap<Name, DefModule>,
    schema_index: &SchemaIndex,
) -> Result<HashMap<Name, WorkflowSchema>, KernelError> {
    let mut map = HashMap::new();
    for (name, module) in modules {
        if let Some(workflow) = module.abi.workflow.as_ref() {
            let schema_name = workflow.event.as_str();
            let event_schema = schema_index
                .get(schema_name)
                .ok_or_else(|| {
                    KernelError::Manifest(format!(
                        "schema '{schema_name}' not found for workflow '{name}'"
                    ))
                })?
                .clone();
            let key_schema = if let Some(key_ref) = &module.key_schema {
                let schema_name = key_ref.as_str();
                Some(
                    schema_index
                        .get(schema_name)
                        .ok_or_else(|| {
                            KernelError::Manifest(format!(
                                "schema '{schema_name}' not found for workflow '{name}' key"
                            ))
                        })?
                        .clone(),
                )
            } else {
                None
            };
            map.insert(
                name.clone(),
                WorkflowSchema {
                    event_schema_name: schema_name.to_string(),
                    event_schema,
                    key_schema,
                },
            );
        }
    }
    Ok(map)
}

fn build_router(
    manifest: &Manifest,
    workflow_schemas: &HashMap<Name, WorkflowSchema>,
) -> Result<HashMap<String, Vec<RouteBinding>>, KernelError> {
    let mut router = HashMap::new();
    let receipt_schema_allows_missing_key_field = |event_schema: &str| {
        matches!(
            event_schema,
            "sys/TimerFired@1" | "sys/BlobPutResult@1" | "sys/BlobGetResult@1"
        )
    };
    let Some(routing) = manifest.routing.as_ref() else {
        return Ok(router);
    };

    for route in &routing.subscriptions {
        let workflow_schema = workflow_schemas.get(&route.module).ok_or_else(|| {
            KernelError::Manifest(format!(
                "schema for workflow '{}' not found while building router",
                route.module
            ))
        })?;
        let route_event = route.event.as_str();
        let workflow_event_schema = workflow_schema.event_schema_name.as_str();
        if route_event == workflow_event_schema {
            push_route_binding(
                &mut router,
                route_event,
                route_event,
                workflow_schema,
                route.key_field.clone(),
                EventWrap::Identity,
                &route.module,
            );
            match &workflow_schema.event_schema {
                TypeExpr::Ref(reference) => {
                    let member = reference.reference.as_str();
                    push_route_binding(
                        &mut router,
                        member,
                        route_event,
                        workflow_schema,
                        route.key_field.clone(),
                        EventWrap::Identity,
                        &route.module,
                    );
                }
                TypeExpr::Variant(variant) => {
                    for (tag, ty) in &variant.variant {
                        if let TypeExpr::Ref(reference) = ty {
                            if route.key_field.is_some()
                                && receipt_schema_allows_missing_key_field(
                                    reference.reference.as_str(),
                                )
                            {
                                continue;
                            }
                            push_route_binding(
                                &mut router,
                                reference.reference.as_str(),
                                route_event,
                                workflow_schema,
                                route.key_field.clone(),
                                EventWrap::Variant { tag: tag.clone() },
                                &route.module,
                            );
                        }
                    }
                }
                _ => {}
            }
        } else {
            let wrap = wrap_for_event_schema(route_event, workflow_schema)?;
            push_route_binding(
                &mut router,
                route_event,
                route_event,
                workflow_schema,
                route.key_field.clone(),
                wrap,
                &route.module,
            );
        }
    }

    Ok(router)
}

fn push_route_binding(
    router: &mut HashMap<String, Vec<RouteBinding>>,
    event_key: &str,
    route_event_schema: &str,
    workflow_schema: &WorkflowSchema,
    key_field: Option<String>,
    wrap: EventWrap,
    workflow: &str,
) {
    router
        .entry(event_key.to_string())
        .or_insert_with(Vec::new)
        .push(RouteBinding {
            workflow: workflow.to_string(),
            key_field,
            route_event_schema: route_event_schema.to_string(),
            workflow_event_schema: workflow_schema.event_schema_name.clone(),
            wrap,
        });
}

fn wrap_for_event_schema(
    event_schema: &str,
    workflow_schema: &WorkflowSchema,
) -> Result<EventWrap, KernelError> {
    if event_schema == workflow_schema.event_schema_name {
        return Ok(EventWrap::Identity);
    }
    match &workflow_schema.event_schema {
        TypeExpr::Ref(reference) if reference.reference.as_str() == event_schema => {
            Ok(EventWrap::Identity)
        }
        TypeExpr::Variant(variant) => {
            let mut found = None;
            for (tag, ty) in &variant.variant {
                if let TypeExpr::Ref(reference) = ty {
                    if reference.reference.as_str() == event_schema {
                        if found.is_some() {
                            return Err(KernelError::Manifest(format!(
                                "event '{event_schema}' appears in multiple variant arms for workflow schema '{}'",
                                workflow_schema.event_schema_name
                            )));
                        }
                        found = Some(tag.clone());
                    }
                }
            }
            found.map(|tag| EventWrap::Variant { tag }).ok_or_else(|| {
                KernelError::Manifest(format!(
                    "event '{event_schema}' is not in workflow schema '{}' family",
                    workflow_schema.event_schema_name
                ))
            })
        }
        _ => Err(KernelError::Manifest(format!(
            "event '{event_schema}' is not in workflow schema '{}' family",
            workflow_schema.event_schema_name
        ))),
    }
}

pub(super) fn persist_loaded_manifest<S: Store>(
    store: &S,
    loaded: &mut LoadedManifest,
) -> Result<(), KernelError> {
    let mut schema_hashes = HashMap::new();
    let mut module_hashes = HashMap::new();
    let mut effect_hashes = HashMap::new();

    for schema in loaded.schemas.values() {
        let hash = store.put_node(&AirNode::Defschema(schema.clone()))?;
        schema_hashes.insert(schema.name.clone(), hash);
    }
    for module in loaded.modules.values() {
        let hash = store.put_node(&AirNode::Defmodule(module.clone()))?;
        module_hashes.insert(module.name.clone(), hash);
    }
    for effect in loaded.effects.values() {
        let hash = store.put_node(&AirNode::Defeffect(effect.clone()))?;
        effect_hashes.insert(effect.name.clone(), hash);
    }

    for reference in loaded.manifest.schemas.iter_mut() {
        if let Some(builtin) = builtins::find_builtin_schema(reference.name.as_str()) {
            reference.hash = builtin.hash_ref.clone();
            continue;
        }
        if let Some(hash) = schema_hashes.get(&reference.name) {
            reference.hash = HashRef::new(hash.to_hex()).map_err(|err| {
                KernelError::Manifest(format!("schema hash '{}': {err}", reference.name))
            })?;
            continue;
        }
        return Err(KernelError::Manifest(format!(
            "manifest references unknown schema '{}'",
            reference.name
        )));
    }

    for reference in loaded.manifest.modules.iter_mut() {
        if let Some(hash) = module_hashes.get(&reference.name) {
            reference.hash = HashRef::new(hash.to_hex()).map_err(|err| {
                KernelError::Manifest(format!("module hash '{}': {err}", reference.name))
            })?;
            continue;
        }
        if let Some(builtin) = builtins::find_builtin_module(reference.name.as_str()) {
            reference.hash = builtin.hash_ref.clone();
            continue;
        }
        return Err(KernelError::Manifest(format!(
            "manifest references unknown module '{}'",
            reference.name
        )));
    }

    for reference in loaded.manifest.effects.iter_mut() {
        if let Some(builtin) = builtins::find_builtin_effect(reference.name.as_str()) {
            reference.hash = builtin.hash_ref.clone();
            continue;
        }
        if let Some(hash) = effect_hashes.get(&reference.name) {
            reference.hash = HashRef::new(hash.to_hex()).map_err(|err| {
                KernelError::Manifest(format!("effect hash '{}': {err}", reference.name))
            })?;
            continue;
        }
        return Err(KernelError::Manifest(format!(
            "manifest references unknown effect '{}'",
            reference.name
        )));
    }

    store.put_node(&loaded.manifest)?;
    store.put_node(&AirNode::Manifest(loaded.manifest.clone()))?;
    Ok(())
}

pub(super) fn extend_schema_map_from_store<S: Store>(
    store: &S,
    refs: &[NamedRef],
    schemas: &mut HashMap<String, TypeExpr>,
) -> Result<(), KernelError> {
    for reference in refs {
        if schemas.contains_key(reference.name.as_str()) {
            continue;
        }
        if let Some(hash) = parse_nonzero_hash(reference.hash.as_str())? {
            let node: AirNode = store.get_node(hash)?;
            if let AirNode::Defschema(schema) = node {
                schemas.insert(schema.name.clone(), schema.ty.clone());
            }
        }
    }
    Ok(())
}

pub(super) fn parse_nonzero_hash(value: &str) -> Result<Option<Hash>, KernelError> {
    let hash = Hash::from_hex_str(value)
        .map_err(|err| KernelError::Manifest(format!("invalid hash '{value}': {err}")))?;
    if hash.as_bytes().iter().all(|b| *b == 0) {
        Ok(None)
    } else {
        Ok(Some(hash))
    }
}
