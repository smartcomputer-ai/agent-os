use super::*;
use aos_air_types::value_normalize::normalize_value_with_schema;
use serde::Serialize;

impl<S: Store + 'static> Kernel<S> {
    const MAX_EFFECTS_PER_TICK: usize = 64;
    const MAX_DOMAIN_EVENTS_PER_TICK: usize = 256;
    const MAX_REDUCER_OUTPUT_BYTES_PER_TICK: usize = 1_048_576;

    pub(super) fn enforce_reducer_output_limits(output: &ReducerOutput) -> Result<(), KernelError> {
        if output.effects.len() > Self::MAX_EFFECTS_PER_TICK {
            return Err(KernelError::ReducerOutput(format!(
                "reducer exceeded max effects per tick: {} > {}",
                output.effects.len(),
                Self::MAX_EFFECTS_PER_TICK
            )));
        }
        if output.domain_events.len() > Self::MAX_DOMAIN_EVENTS_PER_TICK {
            return Err(KernelError::ReducerOutput(format!(
                "reducer exceeded max domain events per tick: {} > {}",
                output.domain_events.len(),
                Self::MAX_DOMAIN_EVENTS_PER_TICK
            )));
        }
        let output_bytes = reducer_output_size_bytes(output);
        if output_bytes > Self::MAX_REDUCER_OUTPUT_BYTES_PER_TICK {
            return Err(KernelError::ReducerOutput(format!(
                "reducer exceeded max output bytes per tick: {output_bytes} > {}",
                Self::MAX_REDUCER_OUTPUT_BYTES_PER_TICK
            )));
        }
        Ok(())
    }

    pub fn submit_domain_event(
        &mut self,
        schema: impl Into<String>,
        value: Vec<u8>,
    ) -> Result<(), KernelError> {
        let event = DomainEvent::new(schema.into(), value);
        self.process_domain_event(event)
    }

    pub fn submit_domain_event_with_key(
        &mut self,
        schema: impl Into<String>,
        value: Vec<u8>,
        key: Vec<u8>,
    ) -> Result<(), KernelError> {
        let event = DomainEvent::with_key(schema.into(), value, key);
        self.process_domain_event(event)
    }

    /// Submit a domain event and surface routing/validation errors (tests/fixtures helper).
    pub fn submit_domain_event_result(
        &mut self,
        schema: impl Into<String>,
        value: Vec<u8>,
    ) -> Result<(), KernelError> {
        self.submit_domain_event(schema, value)
    }

    pub(super) fn process_domain_event(&mut self, event: DomainEvent) -> Result<(), KernelError> {
        let journal_height = self.journal.next_seq();
        let stamp = self.sample_ingress(journal_height)?;
        self.process_domain_event_with_ingress(event, stamp)
    }

    pub(super) fn process_domain_event_with_ingress(
        &mut self,
        event: DomainEvent,
        stamp: IngressStamp,
    ) -> Result<(), KernelError> {
        Self::validate_entropy(&stamp.entropy)?;
        let event = self.normalize_domain_event(event)?;
        let routed = self.route_event(&event, &stamp)?;
        let mut event_for_plans = event.clone();
        if event_for_plans.key.is_none()
            && let Some(key_bytes) = routed.iter().find_map(|ev| ev.event.key.clone())
        {
            event_for_plans.key = Some(key_bytes);
        }
        self.mark_replay_generated_domain_event(&event_for_plans)?;
        self.record_domain_event(&event_for_plans, &stamp)?;
        for ev in routed {
            self.reducer_queue.push_back(ev);
        }
        Ok(())
    }

    fn normalize_domain_event(&self, event: DomainEvent) -> Result<DomainEvent, KernelError> {
        let normalized =
            normalize_cbor_by_name(&self.schema_index, event.schema.as_str(), &event.value)
                .map_err(|err| {
                    KernelError::Manifest(format!(
                        "event '{}' payload failed validation: {err}",
                        event.schema
                    ))
                })?;
        Ok(DomainEvent {
            schema: event.schema,
            value: normalized.bytes,
            key: event.key,
        })
    }

    pub(super) fn route_event(
        &self,
        event: &DomainEvent,
        stamp: &IngressStamp,
    ) -> Result<Vec<ReducerEvent>, KernelError> {
        let mut routed = Vec::new();
        let Some(bindings) = self.router.get(&event.schema) else {
            return Ok(routed);
        };
        let normalized =
            normalize_cbor_by_name(&self.schema_index, event.schema.as_str(), &event.value)
                .map_err(|err| {
                    KernelError::Manifest(format!(
                        "failed to decode event '{}' payload for routing: {err}",
                        event.schema
                    ))
                })?;
        let event_value = normalized.value;
        for binding in bindings {
            let module_def = self
                .module_defs
                .get(&binding.reducer)
                .ok_or_else(|| KernelError::ReducerNotFound(binding.reducer.clone()))?;
            let reducer_schema = self.reducer_schemas.get(&binding.reducer).ok_or_else(|| {
                KernelError::Manifest(format!(
                    "schema for reducer '{}' not found while routing event",
                    binding.reducer
                ))
            })?;
            let keyed = module_def.key_schema.is_some();

            match (keyed, &binding.key_field) {
                (true, None) => {
                    if event.key.is_none() {
                        return Err(KernelError::Manifest(format!(
                            "route to keyed reducer '{}' is missing key_field",
                            binding.reducer
                        )));
                    }
                }
                (false, Some(_)) => {
                    return Err(KernelError::Manifest(format!(
                        "route to non-keyed reducer '{}' provided key_field",
                        binding.reducer
                    )));
                }
                _ => {}
            }

            let wrapped_value = match &binding.wrap {
                EventWrap::Identity => event_value.clone(),
                EventWrap::Variant { tag } => CborValue::Map(BTreeMap::from([
                    (CborValue::Text("$tag".into()), CborValue::Text(tag.clone())),
                    (CborValue::Text("$value".into()), event_value.clone()),
                ])),
            };
            let normalized_for_reducer = normalize_value_with_schema(
                wrapped_value,
                &reducer_schema.event_schema,
                &self.schema_index,
            )
            .map_err(|err| {
                KernelError::Manifest(format!(
                    "failed to encode event '{}' for reducer '{}': {err}",
                    event.schema, binding.reducer
                ))
            })?;

            let key_bytes = if keyed {
                if let Some(field) = &binding.key_field {
                    let key_schema_ref = module_def
                        .key_schema
                        .as_ref()
                        .expect("keyed reducers have key_schema");
                    let key_schema =
                        self.schema_index
                            .get(key_schema_ref.as_str())
                            .ok_or_else(|| {
                                KernelError::Manifest(format!(
                                    "key schema '{}' not found for reducer '{}'",
                                    key_schema_ref.as_str(),
                                    binding.reducer
                                ))
                            })?;
                    let value_for_key = if binding.route_event_schema == event.schema {
                        &event_value
                    } else {
                        &normalized_for_reducer.value
                    };
                    Some(self.extract_key_bytes(
                        field,
                        key_schema,
                        value_for_key,
                        binding.route_event_schema.as_str(),
                    )?)
                } else {
                    event.key.clone()
                }
            } else {
                None
            };

            if let (Some(existing), Some(extracted)) = (&event.key, &key_bytes)
                && existing != extracted
            {
                return Err(KernelError::Manifest(format!(
                    "event '{}' carried key that differs from extracted key for reducer '{}'",
                    event.schema, binding.reducer
                )));
            }

            let mut routed_event = DomainEvent::new(
                binding.reducer_event_schema.clone(),
                normalized_for_reducer.bytes,
            );
            routed_event.key = event.key.clone();
            if let Some(bytes) = key_bytes.clone() {
                routed_event.key = Some(bytes);
            }
            routed.push(ReducerEvent {
                reducer: binding.reducer.clone(),
                event: routed_event,
                stamp: stamp.clone(),
            });
        }
        Ok(routed)
    }

    fn extract_key_bytes(
        &self,
        field: &str,
        key_schema: &TypeExpr,
        event_value: &CborValue,
        event_schema: &str,
    ) -> Result<Vec<u8>, KernelError> {
        let raw_value = extract_cbor_path(event_value, field).ok_or_else(|| {
            KernelError::Manifest(format!(
                "event '{event_schema}' missing key field '{field}'"
            ))
        })?;
        let normalized =
            normalize_value_with_schema(raw_value.clone(), key_schema, &self.schema_index)
                .map_err(|err| {
                    KernelError::Manifest(format!(
                        "event '{event_schema}' key field '{field}' failed validation: {err}"
                    ))
                })?;
        Ok(normalized.bytes)
    }

    pub(super) fn handle_reducer_event(&mut self, event: ReducerEvent) -> Result<(), KernelError> {
        let reducer_name = event.reducer.clone();
        let (keyed, wants_context) = {
            let module_def = self
                .module_defs
                .get(&reducer_name)
                .ok_or_else(|| KernelError::ReducerNotFound(reducer_name.clone()))?;
            if module_def.module_kind != aos_air_types::ModuleKind::Workflow {
                return Err(KernelError::Manifest(format!(
                    "module '{reducer_name}' is not a reducer/workflow module"
                )));
            }
            self.reducers.ensure_loaded(&reducer_name, module_def)?;
            (
                module_def.key_schema.is_some(),
                module_def
                    .abi
                    .reducer
                    .as_ref()
                    .and_then(|abi| abi.context.as_ref())
                    .is_some(),
            )
        };
        let key = event.event.key.clone();
        if keyed && key.is_none() {
            return Err(KernelError::Manifest(format!(
                "reducer '{reducer_name}' is keyed but event '{}' lacked a key",
                event.event.schema
            )));
        }
        if !keyed && key.is_some() {
            return Err(KernelError::Manifest(format!(
                "reducer '{reducer_name}' is not keyed but received a keyed event"
            )));
        }

        let mut index_root = self.reducer_index_roots.get(&reducer_name).copied();
        if keyed {
            index_root = Some(self.ensure_cell_index_root(&reducer_name)?);
        }

        let state_entry = self.reducer_state.entry(reducer_name.clone()).or_default();
        let key_bytes: &[u8] = key.as_deref().unwrap_or(MONO_KEY);
        let current_state = if let Some(entry) = state_entry.cell_cache.get(key_bytes) {
            Some(entry.state.clone())
        } else if let Some(root) = index_root {
            let key_hash = Hash::of_bytes(key_bytes);
            let index = CellIndex::new(self.store.as_ref());
            if let Some(meta) = index.get(root, key_hash.as_bytes())? {
                let state_hash = Hash::from_bytes(&meta.state_hash)
                    .unwrap_or_else(|_| Hash::of_bytes(&meta.state_hash));
                let state = self.store.get_blob(state_hash)?;
                state_entry.cell_cache.insert(
                    key_bytes.to_vec(),
                    CellEntry {
                        state: state.clone(),
                        state_hash,
                        last_active_ns: meta.last_active_ns,
                    },
                );
                Some(state)
            } else {
                None
            }
        } else {
            None
        };

        let ctx_bytes = if wants_context {
            let event_hash = Hash::of_cbor(&event.event)
                .map_err(|err| KernelError::Manifest(err.to_string()))?
                .to_hex();
            let context = aos_wasm_abi::ReducerContext {
                now_ns: event.stamp.now_ns,
                logical_now_ns: event.stamp.logical_now_ns,
                journal_height: event.stamp.journal_height,
                entropy: event.stamp.entropy.clone(),
                event_hash,
                manifest_hash: event.stamp.manifest_hash.clone(),
                reducer: reducer_name.clone(),
                key: key.clone(),
                cell_mode: keyed,
            };
            Some(
                to_canonical_cbor(&context)
                    .map_err(|err| KernelError::Manifest(err.to_string()))?,
            )
        } else {
            None
        };
        self.effect_manager
            .set_cap_context(crate::effects::CapContext {
                logical_now_ns: event.stamp.logical_now_ns,
                journal_height: event.stamp.journal_height,
                manifest_hash: event.stamp.manifest_hash.clone(),
            });
        let input = ReducerInput {
            version: ABI_VERSION,
            state: current_state,
            event: event.event.clone(),
            ctx: ctx_bytes,
        };
        let output = self.reducers.invoke(&reducer_name, &input)?;
        self.handle_reducer_output_with_meta(
            reducer_name.clone(),
            key,
            keyed,
            output,
            event.stamp.journal_height,
        )?;
        Ok(())
    }

    pub(super) fn handle_reducer_output(
        &mut self,
        reducer_name: String,
        key: Option<Vec<u8>>,
        keyed: bool,
        output: ReducerOutput,
    ) -> Result<(), KernelError> {
        let emitted_at_seq = self.journal.next_seq();
        self.handle_reducer_output_with_meta(reducer_name, key, keyed, output, emitted_at_seq)
    }

    fn handle_reducer_output_with_meta(
        &mut self,
        reducer_name: String,
        key: Option<Vec<u8>>,
        keyed: bool,
        output: ReducerOutput,
        emitted_at_seq: u64,
    ) -> Result<(), KernelError> {
        Self::enforce_reducer_output_limits(&output)?;

        let declared_effects = self
            .module_defs
            .get(&reducer_name)
            .and_then(|module| module.abi.reducer.as_ref())
            .map(|abi| abi.effects_emitted.clone())
            .unwrap_or_default();

        let index_root = self.ensure_cell_index_root(&reducer_name)?;
        let mut new_index_root: Option<Hash> = None;

        let entry = self.reducer_state.entry(reducer_name.clone()).or_default();

        let key_bytes = if keyed {
            key.clone().expect("key required for keyed reducer")
        } else {
            MONO_KEY.to_vec()
        };
        let key_hash = Hash::of_bytes(&key_bytes);
        let state_for_workflow = output.state.clone();
        let module_version = self
            .module_defs
            .get(&reducer_name)
            .map(|module| module.wasm_hash.as_str().to_string());

        match output.state {
            Some(state) => {
                let state_hash = self.store.put_blob(&state)?;
                let last_active_ns = self.journal.next_seq() as u64;
                let meta = CellMeta {
                    key_hash: *key_hash.as_bytes(),
                    key_bytes: key_bytes.clone(),
                    state_hash: *state_hash.as_bytes(),
                    size: state.len() as u64,
                    last_active_ns,
                };
                let index = CellIndex::new(self.store.as_ref());
                let new_root = index.upsert(index_root, meta)?;
                new_index_root = Some(new_root);
                entry.cell_cache.insert(
                    key_bytes,
                    CellEntry {
                        state,
                        state_hash,
                        last_active_ns,
                    },
                );
            }
            None => {
                let index = CellIndex::new(self.store.as_ref());
                let (new_root, removed) = index.delete(index_root, key_hash.as_bytes())?;
                if removed {
                    new_index_root = Some(new_root);
                    entry.cell_cache.remove(&key_bytes);
                }
            }
        }
        if let Some(root) = new_index_root {
            self.reducer_index_roots.insert(reducer_name.clone(), root);
        }
        self.record_workflow_state_transition(
            &reducer_name,
            key.as_deref(),
            state_for_workflow.as_deref(),
            emitted_at_seq,
            module_version,
        );
        for event in output.domain_events {
            self.process_domain_event(event)?;
        }
        for (effect_index, effect) in output.effects.iter().enumerate() {
            if !declared_effects
                .iter()
                .any(|kind| kind.as_str() == effect.kind.as_str())
            {
                return Err(KernelError::ReducerOutput(format!(
                    "module '{reducer_name}' emitted undeclared effect kind '{}'; declare it in abi.reducer.effects_emitted",
                    effect.kind
                )));
            }
            let slot = effect.cap_slot.clone().unwrap_or_else(|| "default".into());
            let bound_grant = self
                .module_cap_bindings
                .get(&reducer_name)
                .and_then(|binding| binding.get(&slot));
            let default_grant = if bound_grant.is_none() && slot == "default" {
                self.effect_manager
                    .unique_grant_for_effect_kind(effect.kind.as_str())?
            } else {
                None
            };
            let grant = bound_grant
                .or_else(|| default_grant.as_ref())
                .ok_or_else(|| KernelError::CapabilityBindingMissing {
                    reducer: reducer_name.clone(),
                    slot: slot.clone(),
                })?;
            let mut effect_for_enqueue = effect.clone();
            let derived_idempotency = derive_reducer_intent_idempotency_key(
                reducer_name.as_str(),
                key.as_deref(),
                effect,
                effect_index,
                emitted_at_seq,
            )
            .map_err(KernelError::ReducerOutput)?;
            effect_for_enqueue.idempotency_key = Some(derived_idempotency.to_vec());
            let intent = match self.effect_manager.enqueue_reducer_effect_with_grant(
                &reducer_name,
                grant,
                &effect_for_enqueue,
            ) {
                Ok(intent) => intent,
                Err(err) => {
                    self.record_decisions()?;
                    return Err(err);
                }
            };
            self.record_decisions()?;
            self.record_effect_intent(
                &intent,
                IntentOriginRecord::Reducer {
                    name: reducer_name.clone(),
                    instance_key: key.clone(),
                    emitted_at_seq: Some(emitted_at_seq),
                },
            )?;
            self.pending_reducer_receipts.insert(
                intent.intent_hash,
                ReducerEffectContext::new(
                    reducer_name.clone(),
                    key.clone(),
                    effect.kind.clone(),
                    effect.params_cbor.clone(),
                    intent.intent_hash,
                    emitted_at_seq,
                    self.module_defs
                        .get(&reducer_name)
                        .map(|module| module.wasm_hash.as_str().to_string()),
                ),
            );
            self.record_workflow_inflight_intent(
                &reducer_name,
                key.as_deref(),
                intent.intent_hash,
                effect.kind.as_str(),
                &effect.params_cbor,
                emitted_at_seq,
            );
        }
        Ok(())
    }
}

fn reducer_output_size_bytes(output: &ReducerOutput) -> usize {
    let mut total = 0usize;
    if let Some(state) = &output.state {
        total = total.saturating_add(state.len());
    }
    if let Some(ann) = &output.ann {
        total = total.saturating_add(ann.len());
    }
    for event in &output.domain_events {
        total = total.saturating_add(event.schema.len());
        total = total.saturating_add(event.value.len());
        total = total.saturating_add(event.key.as_ref().map_or(0, Vec::len));
    }
    for effect in &output.effects {
        total = total.saturating_add(effect.kind.len());
        total = total.saturating_add(effect.params_cbor.len());
        total = total.saturating_add(effect.cap_slot.as_ref().map_or(0, String::len));
        total = total.saturating_add(effect.idempotency_key.as_ref().map_or(0, Vec::len));
    }
    total
}

fn derive_reducer_intent_idempotency_key(
    reducer_name: &str,
    reducer_key: Option<&[u8]>,
    effect: &aos_wasm_abi::ReducerEffect,
    effect_index: usize,
    emitted_at_seq: u64,
) -> Result<[u8; 32], String> {
    #[derive(Serialize)]
    struct Preimage<'a> {
        origin_module_id: &'a str,
        #[serde(with = "serde_bytes")]
        origin_instance_key: &'a [u8],
        effect_kind: &'a str,
        #[serde(with = "serde_bytes")]
        params_cbor: &'a [u8],
        #[serde(with = "serde_bytes")]
        requested_idempotency_key: &'a [u8],
        effect_index: u64,
        emitted_at_seq: u64,
    }

    let preimage = Preimage {
        origin_module_id: reducer_name,
        origin_instance_key: reducer_key.unwrap_or_default(),
        effect_kind: effect.kind.as_str(),
        params_cbor: &effect.params_cbor,
        requested_idempotency_key: effect.idempotency_key.as_deref().unwrap_or(&[]),
        effect_index: effect_index as u64,
        emitted_at_seq,
    };
    let bytes = to_canonical_cbor(&preimage).map_err(|err| err.to_string())?;
    let hash = Hash::of_bytes(&bytes);
    Ok(*hash.as_bytes())
}

fn extract_cbor_path(value: &CborValue, path: &str) -> Option<CborValue> {
    let mut current = value;
    for segment in path.split('.') {
        if segment.is_empty() {
            continue;
        }
        current = match current {
            CborValue::Map(map) => map.get(&CborValue::Text(segment.to_string()))?,
            _ => return None,
        };
    }
    Some(current.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::test_support::{
        dummy_stamp, hash, minimal_kernel_keyed_missing_key_field, minimal_kernel_non_keyed,
        minimal_kernel_with_router, minimal_kernel_with_router_non_keyed, schema_event_record,
    };
    use aos_air_types::{
        CURRENT_AIR_VERSION, DefSchema, HashRef, ModuleAbi, ModuleKind, NamedRef, ReducerAbi,
        Routing, RoutingEvent, SchemaRef, TypePrimitive, TypePrimitiveHash, TypePrimitiveText,
        catalog::EffectCatalog,
    };
    use aos_cbor::Hash;
    use aos_wasm_abi::ReducerEffect;
    use indexmap::IndexMap;
    use serde_cbor::Value as CborValue;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    #[test]
    fn route_event_requires_key_for_keyed_reducer() {
        let kernel = minimal_kernel_keyed_missing_key_field();
        let payload = serde_cbor::to_vec(&CborValue::Map(BTreeMap::from([(
            CborValue::Text("id".into()),
            CborValue::Text("1".into()),
        )])))
        .unwrap();
        let event = DomainEvent::new("com.acme/Event@1", payload);
        let err = kernel
            .route_event(&event, &dummy_stamp(&kernel))
            .unwrap_err();
        assert!(format!("{err:?}").contains("missing key_field"), "{err}");
    }

    #[test]
    fn route_event_rejects_key_for_non_keyed_reducer() {
        let kernel = minimal_kernel_with_router_non_keyed();
        let payload = serde_cbor::to_vec(&CborValue::Map(BTreeMap::from([(
            CborValue::Text("id".into()),
            CborValue::Text("1".into()),
        )])))
        .unwrap();
        let event = DomainEvent::new("com.acme/Event@1", payload);
        let err = kernel
            .route_event(&event, &dummy_stamp(&kernel))
            .unwrap_err();
        assert!(format!("{err:?}").contains("provided key_field"), "{err}");
    }

    #[test]
    fn route_event_extracts_key_and_passes_to_reducer() {
        let kernel = minimal_kernel_with_router();
        let payload = serde_cbor::to_vec(&CborValue::Map(BTreeMap::from([(
            CborValue::Text("id".into()),
            CborValue::Text("abc".into()),
        )])))
        .unwrap();
        let event = DomainEvent::new("com.acme/Event@1", payload);
        let routed = kernel
            .route_event(&event, &dummy_stamp(&kernel))
            .expect("route");
        assert_eq!(routed.len(), 1);
        let expected_key = aos_cbor::to_canonical_cbor(&CborValue::Text("abc".into())).unwrap();
        assert_eq!(routed[0].event.key.as_ref().unwrap(), &expected_key);
        assert_eq!(routed[0].reducer, "com.acme/Reducer@1");
    }

    #[test]
    fn event_normalization_rejects_invalid_payload() {
        let mut kernel = minimal_kernel_with_router();
        let payload = serde_cbor::to_vec(&CborValue::Integer(5.into())).unwrap();
        let err = kernel
            .submit_domain_event_result("com.acme/Event@1", payload)
            .unwrap_err();
        assert!(
            matches!(err, KernelError::Manifest(msg) if msg.contains("payload failed validation"))
        );
    }

    #[test]
    fn reducer_output_with_multiple_effects_is_allowed() {
        let output = ReducerOutput {
            effects: vec![
                ReducerEffect::new("timer.set", vec![1]),
                ReducerEffect::new("blob.put", vec![2]),
            ],
            ..Default::default()
        };

        Kernel::<aos_store::MemStore>::enforce_reducer_output_limits(&output).expect("allowed");
    }

    #[test]
    fn reducer_output_effect_limit_is_enforced() {
        let effects = (0..65)
            .map(|_| ReducerEffect::new("timer.set", vec![1]))
            .collect::<Vec<_>>();
        let output = ReducerOutput {
            effects,
            ..Default::default()
        };

        let err =
            Kernel::<aos_store::MemStore>::enforce_reducer_output_limits(&output).unwrap_err();
        assert!(
            matches!(err, KernelError::ReducerOutput(ref message) if message.contains("max effects per tick")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn reducer_output_event_limit_is_enforced() {
        let domain_events = (0..257)
            .map(|_| DomainEvent::new("com.acme/Event@1", vec![0]))
            .collect::<Vec<_>>();
        let output = ReducerOutput {
            domain_events,
            ..Default::default()
        };

        let err =
            Kernel::<aos_store::MemStore>::enforce_reducer_output_limits(&output).unwrap_err();
        assert!(
            matches!(err, KernelError::ReducerOutput(ref message) if message.contains("max domain events per tick")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn reducer_output_bytes_limit_is_enforced() {
        let output = ReducerOutput {
            state: Some(vec![0u8; 1_048_577]),
            ..Default::default()
        };

        let err =
            Kernel::<aos_store::MemStore>::enforce_reducer_output_limits(&output).unwrap_err();
        assert!(
            matches!(err, KernelError::ReducerOutput(ref message) if message.contains("max output bytes per tick")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn reducer_output_rejects_undeclared_effect_kind_before_cap_resolution() {
        let store = Arc::new(aos_store::MemStore::default());
        let journal = Box::new(crate::journal::mem::MemJournal::new());
        let mut kernel =
            crate::world::test_support::kernel_with_store_and_journal(store.clone(), journal);
        let reducer = "com.acme/Reducer@1".to_string();

        let err = kernel
            .handle_reducer_output(
                reducer,
                None,
                false,
                ReducerOutput {
                    effects: vec![ReducerEffect::new("timer.set", vec![1])],
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(
            matches!(err, KernelError::ReducerOutput(ref message) if message.contains("undeclared effect kind")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn intent_key_derivation_includes_instance_identity() {
        let effect = ReducerEffect::new("http.request", vec![1, 2, 3]);
        let key_a = derive_reducer_intent_idempotency_key(
            "com.acme/Workflow@1",
            Some(b"instance-a"),
            &effect,
            0,
            42,
        )
        .expect("derive a");
        let key_b = derive_reducer_intent_idempotency_key(
            "com.acme/Workflow@1",
            Some(b"instance-b"),
            &effect,
            0,
            42,
        )
        .expect("derive b");
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn intent_key_derivation_includes_emission_position() {
        let effect = ReducerEffect::new("http.request", vec![1, 2, 3]);
        let key_a = derive_reducer_intent_idempotency_key(
            "com.acme/Workflow@1",
            Some(b"instance-a"),
            &effect,
            0,
            42,
        )
        .expect("derive a");
        let key_b = derive_reducer_intent_idempotency_key(
            "com.acme/Workflow@1",
            Some(b"instance-a"),
            &effect,
            1,
            42,
        )
        .expect("derive b");
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn cell_index_root_updates_on_upsert_and_delete() {
        let store = Arc::new(aos_store::MemStore::default());
        let journal = Box::new(crate::journal::mem::MemJournal::new());
        let mut kernel =
            crate::world::test_support::kernel_with_store_and_journal(store.clone(), journal);
        let reducer = "com.acme/Reducer@1".to_string();
        let key = b"abc".to_vec();

        kernel
            .handle_reducer_output(
                reducer.clone(),
                Some(key.clone()),
                true,
                ReducerOutput {
                    state: Some(vec![1]),
                    ..Default::default()
                },
            )
            .unwrap();
        let root1 = *kernel.reducer_index_roots.get(&reducer).unwrap();
        let index = CellIndex::new(store.as_ref());
        let meta1 = index
            .get(root1, Hash::of_bytes(&key).as_bytes())
            .unwrap()
            .expect("meta1");
        assert_eq!(meta1.state_hash, *Hash::of_bytes(&[1]).as_bytes());

        kernel
            .handle_reducer_output(
                reducer.clone(),
                Some(key.clone()),
                true,
                ReducerOutput {
                    state: Some(vec![2]),
                    ..Default::default()
                },
            )
            .unwrap();
        let root2 = *kernel.reducer_index_roots.get(&reducer).unwrap();
        assert_ne!(root1, root2);
        let meta2 = index
            .get(root2, Hash::of_bytes(&key).as_bytes())
            .unwrap()
            .expect("meta2");
        assert_eq!(meta2.state_hash, *Hash::of_bytes(&[2]).as_bytes());

        kernel
            .handle_reducer_output(
                reducer.clone(),
                Some(key.clone()),
                true,
                ReducerOutput {
                    state: None,
                    ..Default::default()
                },
            )
            .unwrap();
        let root3 = *kernel.reducer_index_roots.get(&reducer).unwrap();
        assert_ne!(root2, root3);
        let meta3 = index.get(root3, Hash::of_bytes(&key).as_bytes()).unwrap();
        assert!(meta3.is_none());
    }

    #[test]
    fn non_keyed_state_persisted_via_cell_index() {
        let mut kernel = minimal_kernel_non_keyed();
        let reducer = "com.acme/Reducer@1".to_string();
        let state_bytes = b"non-keyed-state".to_vec();
        let output = ReducerOutput {
            state: Some(state_bytes.clone()),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        };

        kernel
            .handle_reducer_output(reducer.clone(), None, false, output)
            .expect("write state");

        let root = kernel.reducer_index_root(&reducer);
        assert!(root.is_some(), "expected index root for non-keyed reducer");
        let cells = kernel.list_cells(&reducer).expect("list cells");
        assert_eq!(cells.len(), 1, "expected sentinel cell entry");
        assert!(
            cells[0].key_bytes.is_empty(),
            "sentinel key should be empty"
        );

        if let Some(entry) = kernel.reducer_state.get_mut(&reducer) {
            entry.cell_cache.remove(MONO_KEY);
        }
        let reloaded = kernel
            .reducer_state_bytes(&reducer, None)
            .expect("read state")
            .expect("state present");
        assert_eq!(reloaded, state_bytes);
    }

    #[test]
    fn reducer_state_traversal_collects_only_typed_hash_refs() {
        let store = aos_store::MemStore::default();
        let module = DefModule {
            name: "com.acme/Reducer@1".into(),
            module_kind: ModuleKind::Workflow,
            wasm_hash: HashRef::new(hash(1)).unwrap(),
            key_schema: None,
            abi: ModuleAbi {
                reducer: Some(ReducerAbi {
                    state: SchemaRef::new("com.acme/StateRefs@1").unwrap(),
                    event: SchemaRef::new("com.acme/Event@1").unwrap(),
                    context: Some(SchemaRef::new("sys/ReducerContext@1").unwrap()),
                    annotations: None,
                    effects_emitted: vec![],
                    cap_slots: Default::default(),
                }),
                pure: None,
            },
        };
        let mut modules = HashMap::new();
        modules.insert(module.name.clone(), module);
        let mut schemas = HashMap::new();
        schemas.insert(
            "com.acme/StateRefs@1".into(),
            DefSchema {
                name: "com.acme/StateRefs@1".into(),
                ty: TypeExpr::Record(aos_air_types::TypeRecord {
                    record: IndexMap::from([
                        (
                            "direct".into(),
                            TypeExpr::Primitive(TypePrimitive::Hash(TypePrimitiveHash {
                                hash: Default::default(),
                            })),
                        ),
                        (
                            "nested".into(),
                            TypeExpr::List(aos_air_types::TypeList {
                                list: Box::new(TypeExpr::Primitive(TypePrimitive::Hash(
                                    TypePrimitiveHash {
                                        hash: Default::default(),
                                    },
                                ))),
                            }),
                        ),
                        (
                            "opaque_text".into(),
                            TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                                text: Default::default(),
                            })),
                        ),
                    ]),
                }),
            },
        );
        schemas.insert(
            "com.acme/Event@1".into(),
            schema_event_record("com.acme/Event@1"),
        );
        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: vec![],
            modules: vec![NamedRef {
                name: "com.acme/Reducer@1".into(),
                hash: HashRef::new(hash(1)).unwrap(),
            }],
            effects: vec![],
            caps: vec![],
            policies: vec![],
            secrets: vec![],
            defaults: None,
            module_bindings: Default::default(),
            routing: Some(Routing {
                subscriptions: vec![RoutingEvent {
                    event: SchemaRef::new("com.acme/Event@1").unwrap(),
                    module: "com.acme/Reducer@1".to_string(),
                    key_field: None,
                }],
                inboxes: vec![],
            }),
        };
        let loaded = LoadedManifest {
            manifest,
            secrets: vec![],
            modules,
            effects: HashMap::new(),
            caps: HashMap::new(),
            policies: HashMap::new(),
            schemas,
            effect_catalog: EffectCatalog::from_defs(Vec::new()),
        };
        let mut kernel = Kernel::from_loaded_manifest(
            Arc::new(store),
            loaded,
            Box::new(crate::journal::mem::MemJournal::default()),
        )
        .unwrap();

        let direct = hash(10);
        let nested = hash(11);
        let opaque = hash(12);
        let state = CborValue::Map(BTreeMap::from([
            (
                CborValue::Text("direct".into()),
                CborValue::Text(direct.clone()),
            ),
            (
                CborValue::Text("nested".into()),
                CborValue::Array(vec![CborValue::Text(nested.clone())]),
            ),
            (
                CborValue::Text("opaque_text".into()),
                CborValue::Text(opaque.clone()),
            ),
        ]));
        kernel
            .handle_reducer_output(
                "com.acme/Reducer@1".into(),
                None,
                false,
                ReducerOutput {
                    state: Some(serde_cbor::to_vec(&state).unwrap()),
                    domain_events: vec![],
                    effects: vec![],
                    ann: None,
                },
            )
            .unwrap();

        let refs = kernel
            .reducer_state_typed_hash_refs("com.acme/Reducer@1", None)
            .unwrap();
        assert!(refs.contains(&Hash::from_hex_str(&direct).unwrap()));
        assert!(refs.contains(&Hash::from_hex_str(&nested).unwrap()));
        assert!(
            !refs.contains(&Hash::from_hex_str(&opaque).unwrap()),
            "opaque text hashes must not be auto-traversed"
        );
    }
}
