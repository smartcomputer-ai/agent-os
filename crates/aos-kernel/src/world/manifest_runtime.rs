use std::collections::HashMap;
use std::sync::Arc;

use aos_air_types::{
    AirNode, DefCap, DefModule, DefPlan, DefPolicy, HashRef, Manifest, Name, NamedRef,
    PlanStepKind, TypeExpr, TypePrimitive, builtins, catalog::EffectCatalog,
    plan_literals::SchemaIndex,
};
use aos_cbor::Hash;
use aos_store::Store;

use crate::capability::{CapGrantResolution, CapabilityResolver};
use crate::error::KernelError;
use crate::manifest::LoadedManifest;
use crate::plan::{PlanRegistry, ReducerSchema};
use crate::policy::{AllowAllPolicy, PolicyGate, RulePolicy};

use super::{EventWrap, PlanTriggerBinding, RouteBinding};

pub(super) struct RuntimeAssembly {
    pub schema_index: Arc<SchemaIndex>,
    pub reducer_schemas: Arc<HashMap<Name, ReducerSchema>>,
    pub router: HashMap<String, Vec<RouteBinding>>,
    pub plan_registry: PlanRegistry,
    pub plan_triggers: HashMap<String, Vec<PlanTriggerBinding>>,
    pub plan_cap_handles: HashMap<Name, Arc<HashMap<String, CapGrantResolution>>>,
    pub module_cap_bindings: HashMap<Name, HashMap<String, CapGrantResolution>>,
    pub capability_resolver: CapabilityResolver,
    pub policy_gate: Box<dyn PolicyGate>,
    pub effect_catalog: Arc<EffectCatalog>,
}

pub(super) fn assemble_runtime<S: Store>(
    store: &S,
    loaded: &LoadedManifest,
) -> Result<RuntimeAssembly, KernelError> {
    let schema_index = Arc::new(build_schema_index_from_loaded(store, loaded)?);
    let reducer_schemas = Arc::new(build_reducer_schemas(
        &loaded.modules,
        schema_index.as_ref(),
    )?);
    let router = build_router(&loaded.manifest, reducer_schemas.as_ref())?;
    let plan_registry = build_plan_registry(&loaded.plans);
    let plan_triggers = build_plan_triggers(&loaded.manifest);
    let effect_catalog = Arc::new(loaded.effect_catalog.clone());
    let capability_resolver = CapabilityResolver::from_manifest(
        &loaded.manifest,
        &loaded.caps,
        schema_index.as_ref(),
        effect_catalog.clone(),
    )?;
    let plan_cap_handles = resolve_plan_cap_handles(&loaded.plans, &capability_resolver)?;
    let module_cap_bindings = resolve_module_cap_bindings(&loaded.manifest, &capability_resolver)?;
    let policy_gate = build_policy_gate(&loaded.manifest, &loaded.policies)?;

    Ok(RuntimeAssembly {
        schema_index,
        reducer_schemas,
        router,
        plan_registry,
        plan_triggers,
        plan_cap_handles,
        module_cap_bindings,
        capability_resolver,
        policy_gate,
        effect_catalog,
    })
}

fn build_plan_registry(plans: &HashMap<Name, DefPlan>) -> PlanRegistry {
    let mut registry = PlanRegistry::default();
    for plan in plans.values() {
        registry.register(plan.clone());
    }
    registry
}

fn build_plan_triggers(manifest: &Manifest) -> HashMap<String, Vec<PlanTriggerBinding>> {
    let mut plan_triggers = HashMap::new();
    for trigger in &manifest.triggers {
        plan_triggers
            .entry(trigger.event.as_str().to_string())
            .or_insert_with(Vec::new)
            .push(PlanTriggerBinding {
                plan: trigger.plan.clone(),
                correlate_by: trigger.correlate_by.clone(),
                when: trigger.when.clone(),
                input_expr: trigger.input_expr.clone(),
            });
    }
    for bindings in plan_triggers.values_mut() {
        bindings.sort_by(|a, b| {
            a.plan
                .cmp(&b.plan)
                .then_with(|| a.correlate_by.cmp(&b.correlate_by))
        });
    }
    plan_triggers
}

fn build_policy_gate(
    manifest: &Manifest,
    policies: &HashMap<Name, DefPolicy>,
) -> Result<Box<dyn PolicyGate>, KernelError> {
    match manifest
        .defaults
        .as_ref()
        .and_then(|defaults| defaults.policy.clone())
    {
        Some(policy_name) => {
            let def = policies.get(&policy_name).ok_or_else(|| {
                KernelError::Manifest(format!(
                    "policy '{policy_name}' referenced by manifest defaults was not found"
                ))
            })?;
            Ok(Box::new(RulePolicy::from_def(def)))
        }
        None => Ok(Box::new(AllowAllPolicy)),
    }
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

fn build_reducer_schemas(
    modules: &HashMap<Name, DefModule>,
    schema_index: &SchemaIndex,
) -> Result<HashMap<Name, ReducerSchema>, KernelError> {
    let mut map = HashMap::new();
    for (name, module) in modules {
        if let Some(reducer) = module.abi.reducer.as_ref() {
            let schema_name = reducer.event.as_str();
            let event_schema = schema_index
                .get(schema_name)
                .ok_or_else(|| {
                    KernelError::Manifest(format!(
                        "schema '{schema_name}' not found for reducer '{name}'"
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
                                "schema '{schema_name}' not found for reducer '{name}' key"
                            ))
                        })?
                        .clone(),
                )
            } else {
                None
            };
            map.insert(
                name.clone(),
                ReducerSchema {
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
    reducer_schemas: &HashMap<Name, ReducerSchema>,
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

    for route in &routing.events {
        let reducer_schema = reducer_schemas.get(&route.reducer).ok_or_else(|| {
            KernelError::Manifest(format!(
                "schema for reducer '{}' not found while building router",
                route.reducer
            ))
        })?;
        let route_event = route.event.as_str();
        let reducer_event_schema = reducer_schema.event_schema_name.as_str();
        if route_event == reducer_event_schema {
            push_route_binding(
                &mut router,
                route_event,
                route_event,
                reducer_schema,
                route.key_field.clone(),
                EventWrap::Identity,
                &route.reducer,
            );
            match &reducer_schema.event_schema {
                TypeExpr::Ref(reference) => {
                    let member = reference.reference.as_str();
                    push_route_binding(
                        &mut router,
                        member,
                        route_event,
                        reducer_schema,
                        route.key_field.clone(),
                        EventWrap::Identity,
                        &route.reducer,
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
                                reducer_schema,
                                route.key_field.clone(),
                                EventWrap::Variant { tag: tag.clone() },
                                &route.reducer,
                            );
                        }
                    }
                }
                _ => {}
            }
        } else {
            let wrap = wrap_for_event_schema(route_event, reducer_schema)?;
            push_route_binding(
                &mut router,
                route_event,
                route_event,
                reducer_schema,
                route.key_field.clone(),
                wrap,
                &route.reducer,
            );
        }
    }

    Ok(router)
}

fn push_route_binding(
    router: &mut HashMap<String, Vec<RouteBinding>>,
    event_key: &str,
    route_event_schema: &str,
    reducer_schema: &ReducerSchema,
    key_field: Option<String>,
    wrap: EventWrap,
    reducer: &str,
) {
    router
        .entry(event_key.to_string())
        .or_insert_with(Vec::new)
        .push(RouteBinding {
            reducer: reducer.to_string(),
            key_field,
            route_event_schema: route_event_schema.to_string(),
            reducer_event_schema: reducer_schema.event_schema_name.clone(),
            wrap,
        });
}

fn wrap_for_event_schema(
    event_schema: &str,
    reducer_schema: &ReducerSchema,
) -> Result<EventWrap, KernelError> {
    if event_schema == reducer_schema.event_schema_name {
        return Ok(EventWrap::Identity);
    }
    match &reducer_schema.event_schema {
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
                                "event '{event_schema}' appears in multiple variant arms for reducer schema '{}'",
                                reducer_schema.event_schema_name
                            )));
                        }
                        found = Some(tag.clone());
                    }
                }
            }
            found.map(|tag| EventWrap::Variant { tag }).ok_or_else(|| {
                KernelError::Manifest(format!(
                    "event '{event_schema}' is not in reducer schema '{}' family",
                    reducer_schema.event_schema_name
                ))
            })
        }
        _ => Err(KernelError::Manifest(format!(
            "event '{event_schema}' is not in reducer schema '{}' family",
            reducer_schema.event_schema_name
        ))),
    }
}

fn resolve_plan_cap_handles(
    plans: &HashMap<Name, DefPlan>,
    resolver: &CapabilityResolver,
) -> Result<HashMap<Name, Arc<HashMap<String, CapGrantResolution>>>, KernelError> {
    let mut plan_caps = HashMap::new();
    for plan in plans.values() {
        for cap in &plan.required_caps {
            if !resolver.has_grant(cap) {
                return Err(KernelError::PlanCapabilityMissing {
                    plan: plan.name.clone(),
                    cap: cap.clone(),
                });
            }
        }
        let mut step_caps = HashMap::new();
        for step in &plan.steps {
            if let PlanStepKind::EmitEffect(emit) = &step.kind {
                let resolved = resolver.resolve(emit.cap.as_str(), emit.kind.as_str())?;
                step_caps.insert(step.id.clone(), resolved);
            }
        }
        plan_caps.insert(plan.name.clone(), Arc::new(step_caps));
    }
    Ok(plan_caps)
}

fn resolve_module_cap_bindings(
    manifest: &Manifest,
    resolver: &CapabilityResolver,
) -> Result<HashMap<Name, HashMap<String, CapGrantResolution>>, KernelError> {
    let mut bindings = HashMap::new();
    for (module, binding) in &manifest.module_bindings {
        let mut slot_map = HashMap::new();
        for (slot, cap) in &binding.slots {
            if !resolver.has_grant(cap) {
                return Err(KernelError::ModuleCapabilityMissing {
                    module: module.clone(),
                    cap: cap.clone(),
                });
            }
            let resolved = resolver.resolve_grant(cap)?;
            slot_map.insert(slot.clone(), resolved);
        }
        bindings.insert(module.clone(), slot_map);
    }
    Ok(bindings)
}

pub(super) fn persist_loaded_manifest<S: Store>(
    store: &S,
    loaded: &mut LoadedManifest,
) -> Result<(), KernelError> {
    let mut schema_hashes = HashMap::new();
    let mut module_hashes = HashMap::new();
    let mut plan_hashes = HashMap::new();
    let mut effect_hashes = HashMap::new();
    let mut cap_hashes = HashMap::new();
    let mut policy_hashes = HashMap::new();

    for schema in loaded.schemas.values() {
        let hash = store.put_node(&AirNode::Defschema(schema.clone()))?;
        schema_hashes.insert(schema.name.clone(), hash);
    }
    for module in loaded.modules.values() {
        let hash = store.put_node(&AirNode::Defmodule(module.clone()))?;
        module_hashes.insert(module.name.clone(), hash);
    }
    for plan in loaded.plans.values() {
        let hash = store.put_node(&AirNode::Defplan(plan.clone()))?;
        plan_hashes.insert(plan.name.clone(), hash);
    }
    for cap in loaded.caps.values() {
        let hash = store.put_node(&AirNode::Defcap(cap.clone()))?;
        cap_hashes.insert(cap.name.clone(), hash);
    }
    for policy in loaded.policies.values() {
        let hash = store.put_node(&AirNode::Defpolicy(policy.clone()))?;
        policy_hashes.insert(policy.name.clone(), hash);
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

    for reference in loaded.manifest.plans.iter_mut() {
        if let Some(hash) = plan_hashes.get(&reference.name) {
            reference.hash = HashRef::new(hash.to_hex()).map_err(|err| {
                KernelError::Manifest(format!("plan hash '{}': {err}", reference.name))
            })?;
        } else {
            return Err(KernelError::Manifest(format!(
                "manifest references unknown plan '{}'",
                reference.name
            )));
        }
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

    for reference in loaded.manifest.caps.iter_mut() {
        if let Some(builtin) = builtins::find_builtin_cap(reference.name.as_str()) {
            reference.hash = builtin.hash_ref.clone();
            continue;
        }
        if let Some(hash) = cap_hashes.get(&reference.name) {
            reference.hash = HashRef::new(hash.to_hex()).map_err(|err| {
                KernelError::Manifest(format!("cap hash '{}': {err}", reference.name))
            })?;
            continue;
        }
        return Err(KernelError::Manifest(format!(
            "manifest references unknown cap '{}'",
            reference.name
        )));
    }

    for reference in loaded.manifest.policies.iter_mut() {
        if let Some(hash) = policy_hashes.get(&reference.name) {
            reference.hash = HashRef::new(hash.to_hex()).map_err(|err| {
                KernelError::Manifest(format!("policy hash '{}': {err}", reference.name))
            })?;
        } else {
            return Err(KernelError::Manifest(format!(
                "manifest references unknown policy '{}'",
                reference.name
            )));
        }
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
