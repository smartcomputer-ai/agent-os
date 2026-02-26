use super::*;
use indexmap::IndexMap;

impl<S: Store + 'static> Kernel<S> {
    pub fn tick(&mut self) -> Result<(), KernelError> {
        if let Some(task) = self.scheduler.pop() {
            match task {
                Task::Reducer(event) => self.handle_reducer_event(event)?,
            }
        }
        Ok(())
    }

    pub fn tick_until_idle(&mut self) -> Result<(), KernelError> {
        while !self.scheduler.is_empty() {
            self.tick()?;
        }
        Ok(())
    }

    pub fn drain_effects(&mut self) -> Result<Vec<aos_effects::EffectIntent>, KernelError> {
        self.effect_manager.drain()
    }

    /// Returns true when the effect queue is non-empty and a cycle is needed.
    pub fn has_pending_effects(&self) -> bool {
        self.effect_manager.has_pending()
    }

    pub fn restore_effect_queue(&mut self, intents: Vec<aos_effects::EffectIntent>) {
        self.effect_manager.restore_queue(intents);
    }

    pub(super) fn start_plans_for_event(
        &mut self,
        event: &DomainEvent,
        stamp: &IngressStamp,
    ) -> Result<(), KernelError> {
        if let Some(plan_bindings) = self.plan_triggers.get(&event.schema).cloned() {
            let event_schema = self
                .schema_index
                .get(event.schema.as_str())
                .ok_or_else(|| {
                    KernelError::Manifest(format!(
                        "trigger event schema '{}' not found",
                        event.schema
                    ))
                })?;
            let normalized_event =
                normalize_cbor_by_name(&self.schema_index, event.schema.as_str(), &event.value)
                    .map_err(|err| {
                        KernelError::Manifest(format!(
                            "failed to decode trigger event '{}' payload: {err}",
                            event.schema
                        ))
                    })?;
            let event_value =
                cbor_to_expr_value(&normalized_event.value, event_schema, &self.schema_index)?;

            for binding in &plan_bindings {
                if let Some(plan_def) = self.plan_registry.get(&binding.plan).cloned() {
                    let mut trigger_env = aos_air_exec::Env::new(ExprValue::Unit);
                    trigger_env.current_event = Some(event_value.clone());

                    if let Some(predicate) = binding.when.as_ref() {
                        let passes =
                            aos_air_exec::eval_expr(predicate, &trigger_env).map_err(|err| {
                                KernelError::Manifest(format!(
                                    "trigger when eval error for event '{}' -> plan '{}': {err}",
                                    event.schema, binding.plan
                                ))
                            })?;
                        if !crate::plan::value_to_bool(passes)? {
                            continue;
                        }
                    }

                    let trigger_input = if let Some(input_expr) = binding.input_expr.as_ref() {
                        crate::plan::eval_expr_or_value(
                            input_expr,
                            &trigger_env,
                            "trigger input_expr eval error",
                        )?
                    } else {
                        event_value.clone()
                    };

                    let input_schema =
                        self.schema_index
                            .get(plan_def.input.as_str())
                            .ok_or_else(|| {
                                KernelError::Manifest(format!(
                                    "plan '{}' input schema '{}' not found",
                                    plan_def.name, plan_def.input
                                ))
                            })?;
                    let trigger_input_cbor = crate::plan::expr_value_to_cbor_value(&trigger_input);
                    let normalized_input = normalize_value_with_schema(
                        trigger_input_cbor,
                        input_schema,
                        &self.schema_index,
                    )
                    .map_err(|err| {
                        KernelError::Manifest(format!(
                            "trigger input failed schema validation for event '{}' -> plan '{}': {err}",
                            event.schema, binding.plan
                        ))
                    })?;
                    let input = cbor_to_expr_value(
                        &normalized_input.value,
                        input_schema,
                        &self.schema_index,
                    )?;
                    let correlation =
                        determine_correlation_value(binding, &input, event.key.as_ref());
                    let instance_id = self.start_plan_instance(
                        &plan_def.name,
                        input,
                        crate::plan::PlanContext::from_stamp(stamp),
                        correlation,
                        None,
                    )?;
                    self.scheduler.push_plan(instance_id);
                } else {
                    log::warn!(
                        "trigger event '{}' references missing plan '{}'",
                        event.schema,
                        binding.plan
                    );
                }
            }
        }
        Ok(())
    }

    fn handle_plan_task(&mut self, id: u64) -> Result<(), KernelError> {
        let waiting_schema = self
            .plan_instances
            .get(&id)
            .and_then(|inst| inst.waiting_event_schema())
            .map(|s| s.to_string());
        if let Some(schema) = waiting_schema {
            self.remove_plan_from_waiting_events_for_schema(id, &schema);
        }
        if self.plan_instances.contains_key(&id) {
            let (plan_name, plan_context, outcome) = {
                let instance = self
                    .plan_instances
                    .get_mut(&id)
                    .expect("instance must exist");
                let name = instance.name.clone();
                let context = instance.context().cloned();
                if let Some(context) = instance.context().cloned() {
                    self.effect_manager
                        .set_cap_context(crate::effects::CapContext {
                            logical_now_ns: context.logical_now_ns,
                            journal_height: context.journal_height,
                            manifest_hash: context.manifest_hash.clone(),
                        });
                } else {
                    self.effect_manager.clear_cap_context();
                }
                let outcome = match instance.tick(&mut self.effect_manager) {
                    Ok(outcome) => outcome,
                    Err(err) => {
                        self.record_decisions()?;
                        return Err(err);
                    }
                };
                (name, context, outcome)
            };
            self.record_decisions()?;
            for event in &outcome.raised_events {
                self.process_domain_event(event.clone())?;
            }
            let mut intent_kinds = HashMap::new();
            for intent in &outcome.intents_enqueued {
                self.record_effect_intent(
                    intent,
                    IntentOriginRecord::Plan {
                        name: plan_name.clone(),
                        plan_id: id,
                    },
                )?;
                intent_kinds.insert(intent.intent_hash, intent.kind.as_str().to_string());
            }
            for hash in &outcome.waiting_receipts {
                let kind = intent_kinds.get(hash).cloned().or_else(|| {
                    self.effect_manager
                        .queued()
                        .iter()
                        .find(|intent| intent.intent_hash == *hash)
                        .map(|intent| intent.kind.as_str().to_string())
                });
                self.pending_receipts.insert(
                    *hash,
                    PendingPlanReceiptInfo {
                        plan_id: id,
                        effect_kind: kind.unwrap_or_else(|| "unknown".into()),
                    },
                );
            }
            let Some(plan_context) = plan_context else {
                return Err(KernelError::Manifest(format!(
                    "plan '{plan_name}' missing execution context"
                )));
            };
            for request in &outcome.spawn_requests {
                let delivered =
                    self.handle_spawn_request(id, request, &plan_context, plan_name.as_str())?;
                if delivered && self.plan_instances.contains_key(&id) {
                    self.scheduler.push_plan(id);
                }
            }
            for request in &outcome.wait_requests {
                self.handle_plan_wait_request(id, request)?;
            }
            if let Some(schema) = outcome.waiting_event.clone() {
                self.waiting_events.entry(schema).or_default().push(id);
            }
            if outcome.completed {
                let completion = self.build_plan_completion_value(&outcome);
                self.remember_plan_completion(id, completion);
                if !self.suppress_journal {
                    let status = if outcome.plan_error.is_some() {
                        PlanEndStatus::Error
                    } else {
                        PlanEndStatus::Ok
                    };
                    let ended = PlanEndedRecord {
                        plan_name: plan_name.clone(),
                        plan_id: id,
                        status: status.clone(),
                        error_code: outcome.plan_error.as_ref().map(|err| err.code.clone()),
                    };
                    self.record_plan_ended(ended)?;
                    if status == PlanEndStatus::Ok {
                        if let (Some(value_cbor), Some(output_schema)) =
                            (outcome.result_cbor.clone(), outcome.result_schema.clone())
                        {
                            let entry = PlanResultEntry::new(
                                plan_name.clone(),
                                id,
                                output_schema,
                                value_cbor,
                            );
                            self.record_plan_result(&entry)?;
                            self.push_plan_result_entry(entry);
                        }
                    }
                }
                self.plan_instances.remove(&id);
                self.wake_plan_waiters(id);
            } else if outcome.waiting_receipts.is_empty()
                && outcome.waiting_event.is_none()
                && !outcome.waiting_plans
                && outcome.spawn_requests.is_empty()
                && outcome.wait_requests.is_empty()
            {
                self.scheduler.push_plan(id);
            }
        }
        Ok(())
    }

    pub fn handle_receipt(
        &mut self,
        receipt: aos_effects::EffectReceipt,
    ) -> Result<(), KernelError> {
        let journal_height = self.journal.next_seq();
        let stamp = self.sample_ingress(journal_height)?;
        self.handle_receipt_with_ingress(receipt, stamp)
    }

    pub(super) fn handle_receipt_with_ingress(
        &mut self,
        receipt: aos_effects::EffectReceipt,
        stamp: IngressStamp,
    ) -> Result<(), KernelError> {
        Self::validate_entropy(&stamp.entropy)?;

        if self.recent_receipt_index.contains(&receipt.intent_hash) {
            log::warn!(
                "late receipt {} ignored (already applied)",
                format_intent_hash(&receipt.intent_hash)
            );
            return Ok(());
        }

        if let Some(context) = self.pending_reducer_receipts.remove(&receipt.intent_hash) {
            let receipt = self.normalize_receipt_payload_for_effect(receipt, &context)?;
            self.record_effect_receipt(&receipt, &stamp)?;
            self.record_decisions()?;
            self.deliver_receipt_to_workflow_instance(context, &receipt, &stamp)?;
            self.remember_receipt(receipt.intent_hash);
            return Ok(());
        }

        if self.suppress_journal {
            log::warn!(
                "receipt {} ignored during replay (no pending context)",
                format_intent_hash(&receipt.intent_hash)
            );
            return Ok(());
        }

        Err(KernelError::UnknownReceipt(format_intent_hash(
            &receipt.intent_hash,
        )))
    }

    fn normalize_receipt_payload_for_effect(
        &self,
        mut receipt: aos_effects::EffectReceipt,
        context: &ReducerEffectContext,
    ) -> Result<aos_effects::EffectReceipt, KernelError> {
        let receipt_schema = self
            .effect_defs
            .values()
            .find(|def| def.kind.as_str() == context.effect_kind)
            .map(|def| def.receipt_schema.as_str().to_string())
            .ok_or_else(|| KernelError::UnsupportedEffectKind(context.effect_kind.clone()))?;
        let normalized = normalize_cbor_by_name(
            &self.schema_index,
            &receipt_schema,
            &receipt.payload_cbor,
        )
        .map_err(|err| {
            KernelError::ReceiptDecode(format!(
                "receipt payload for '{}' failed schema '{}': {err}",
                context.effect_kind, receipt_schema
            ))
        })?;
        receipt.payload_cbor = normalized.bytes;
        Ok(receipt)
    }

    fn deliver_receipt_to_workflow_instance(
        &mut self,
        context: ReducerEffectContext,
        receipt: &aos_effects::EffectReceipt,
        stamp: &IngressStamp,
    ) -> Result<(), KernelError> {
        let reducer_name = context.origin_module_id.clone();
        let module_def = self
            .module_defs
            .get(&reducer_name)
            .ok_or_else(|| KernelError::ReducerNotFound(reducer_name.clone()))?;
        let reducer_abi = module_def.abi.reducer.as_ref().ok_or_else(|| {
            KernelError::Manifest(format!("module '{reducer_name}' is not a reducer/workflow"))
        })?;
        let reducer_event_schema_name = reducer_abi.event.as_str().to_string();
        let reducer_event_schema = self
            .schema_index
            .get(reducer_event_schema_name.as_str())
            .ok_or_else(|| {
                KernelError::Manifest(format!(
                    "schema '{}' not found for workflow module '{}'",
                    reducer_event_schema_name, reducer_name
                ))
            })?;

        let generic_event = crate::receipts::build_workflow_receipt_event(&context, receipt)?;
        let generic_value: CborValue = serde_cbor::from_slice(&generic_event.value)
            .map_err(|err| KernelError::ReceiptDecode(err.to_string()))?;
        let normalized = match try_normalize_receipt_payload(
            reducer_event_schema,
            &self.schema_index,
            generic_value.clone(),
            crate::receipts::SYS_EFFECT_RECEIPT_ENVELOPE_SCHEMA,
        ) {
            Ok(normalized) => normalized,
            Err(generic_err) => {
                if let Some(legacy_event) =
                    crate::receipts::build_legacy_reducer_receipt_event(&context, receipt)?
                {
                    let legacy_value: CborValue = serde_cbor::from_slice(&legacy_event.value)
                        .map_err(|err| KernelError::ReceiptDecode(err.to_string()))?;
                    try_normalize_receipt_payload(
                        reducer_event_schema,
                        &self.schema_index,
                        legacy_value,
                        legacy_event.schema.as_str(),
                    )
                    .map_err(|legacy_err| {
                        KernelError::Manifest(format!(
                            "receipt payload for '{reducer_name}' does not match event schema '{}': generic={generic_err}; legacy={legacy_err}",
                            reducer_event_schema_name
                        ))
                    })?
                } else {
                    return Err(KernelError::Manifest(format!(
                        "receipt payload for '{reducer_name}' does not match event schema '{}': {generic_err}",
                        reducer_event_schema_name
                    )));
                }
            }
        };

        let mut event = DomainEvent::new(reducer_event_schema_name, normalized.bytes);
        event.key = context.origin_instance_key.clone();
        self.scheduler.push_reducer(ReducerEvent {
            reducer: reducer_name.clone(),
            event,
            stamp: stamp.clone(),
        });
        self.mark_workflow_receipt_settled(
            &reducer_name,
            context.origin_instance_key.as_deref(),
            receipt.intent_hash,
        );
        Ok(())
    }

    pub(super) fn deliver_event_to_waiting_plans(
        &mut self,
        event: &DomainEvent,
        stamp: &IngressStamp,
    ) -> Result<(), KernelError> {
        if let Some(mut plan_ids) = self.waiting_events.remove(&event.schema) {
            let mut still_waiting = Vec::new();
            for id in plan_ids.drain(..) {
                if let Some(instance) = self.plan_instances.get_mut(&id) {
                    instance.set_context(crate::plan::PlanContext::from_stamp(stamp));
                    if instance.deliver_event(event)? {
                        self.scheduler.push_plan(id);
                    } else {
                        still_waiting.push(id);
                    }
                }
            }
            if !still_waiting.is_empty() {
                self.waiting_events
                    .insert(event.schema.clone(), still_waiting);
            }
        }
        Ok(())
    }

    fn remove_plan_from_waiting_events_for_schema(&mut self, plan_id: u64, schema: &str) {
        if let Some(ids) = self.waiting_events.get_mut(schema) {
            if let Some(pos) = ids.iter().position(|id| *id == plan_id) {
                ids.swap_remove(pos);
            }
            if ids.is_empty() {
                self.waiting_events.remove(schema);
            }
        }
    }

    fn remember_receipt(&mut self, hash: [u8; 32]) {
        if self.recent_receipt_index.contains(&hash) {
            return;
        }
        if self.recent_receipts.len() >= RECENT_RECEIPT_CACHE {
            if let Some(old) = self.recent_receipts.pop_front() {
                self.recent_receipt_index.remove(&old);
            }
        }
        self.recent_receipts.push_back(hash);
        self.recent_receipt_index.insert(hash);
    }

    pub(super) fn push_plan_result_entry(&mut self, entry: PlanResultEntry) {
        if self.plan_results.len() >= RECENT_PLAN_RESULT_CACHE {
            self.plan_results.pop_front();
        }
        self.plan_results.push_back(entry);
    }

    fn record_plan_result(&mut self, entry: &PlanResultEntry) -> Result<(), KernelError> {
        let record = entry.to_record();
        self.append_record(JournalRecord::PlanResult(record))
    }

    fn record_plan_started(&mut self, record: PlanStartedRecord) -> Result<(), KernelError> {
        self.append_record(JournalRecord::PlanStarted(record))
    }

    fn record_plan_ended(&mut self, record: PlanEndedRecord) -> Result<(), KernelError> {
        self.append_record(JournalRecord::PlanEnded(record))
    }

    fn start_plan_instance(
        &mut self,
        plan_name: &str,
        input: ExprValue,
        context: crate::plan::PlanContext,
        correlation: Option<(Vec<u8>, ExprValue)>,
        parent_instance_id: Option<u64>,
    ) -> Result<u64, KernelError> {
        let plan_def = self
            .plan_registry
            .get(plan_name)
            .ok_or_else(|| KernelError::Manifest(format!("unknown child plan '{plan_name}'")))?;
        let cap_handles = self
            .plan_cap_handles
            .get(&plan_def.name)
            .ok_or_else(|| {
                KernelError::Manifest(format!("plan '{}' missing cap bindings", plan_def.name))
            })?
            .clone();
        let input_hash = {
            let cbor = to_canonical_cbor(&crate::plan::expr_value_to_cbor_value(&input))
                .map_err(|err| KernelError::Manifest(err.to_string()))?;
            aos_cbor::Hash::of_bytes(&cbor).to_hex()
        };
        let plan_id = self.scheduler.alloc_plan_id();
        let mut instance = PlanInstance::new(
            plan_id,
            plan_def.clone(),
            input,
            self.schema_index.clone(),
            correlation,
            cap_handles,
        );
        instance.set_context(context);
        self.plan_instances.insert(plan_id, instance);
        self.record_plan_started(PlanStartedRecord {
            plan_name: plan_name.to_string(),
            plan_id,
            input_hash,
            parent_instance_id,
        })?;
        Ok(plan_id)
    }

    fn coerce_input_for_plan(
        &self,
        plan_name: &str,
        input: ExprValue,
    ) -> Result<ExprValue, KernelError> {
        let plan = self
            .plan_registry
            .get(plan_name)
            .ok_or_else(|| KernelError::Manifest(format!("unknown plan '{plan_name}'")))?;
        let schema = self.schema_index.get(plan.input.as_str()).ok_or_else(|| {
            KernelError::Manifest(format!(
                "plan '{}' input schema '{}' not found",
                plan.name, plan.input
            ))
        })?;
        let normalized = normalize_value_with_schema(
            crate::plan::expr_value_to_cbor_value(&input),
            schema,
            &self.schema_index,
        )
        .map_err(|err| {
            KernelError::Manifest(format!(
                "spawn input failed schema validation for child plan '{}': {err}",
                plan.name
            ))
        })?;
        cbor_to_expr_value(&normalized.value, schema, &self.schema_index)
    }

    fn handle_spawn_request(
        &mut self,
        parent_plan_id: u64,
        request: &crate::plan::PlanSpawnRequest,
        parent_context: &crate::plan::PlanContext,
        _parent_plan_name: &str,
    ) -> Result<bool, KernelError> {
        match request {
            crate::plan::PlanSpawnRequest::SpawnPlan {
                step_id,
                child_plan,
                input,
            } => {
                let input = self.coerce_input_for_plan(child_plan, input.clone())?;
                let child_id = self.start_plan_instance(
                    child_plan,
                    input,
                    parent_context.clone(),
                    None,
                    Some(parent_plan_id),
                )?;
                self.scheduler.push_plan(child_id);
                if let Some(parent) = self.plan_instances.get_mut(&parent_plan_id) {
                    let handle_value = plan_handle_expr_value(child_id, child_plan.clone());
                    if parent.deliver_spawn_value(step_id, handle_value) {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            crate::plan::PlanSpawnRequest::SpawnForEach {
                step_id,
                child_plan,
                inputs,
                max_fanout,
            } => {
                if let Some(limit) = max_fanout
                    && inputs.len() > *limit as usize
                {
                    return Err(KernelError::Manifest(format!(
                        "spawn_for_each max_fanout exceeded: {} > {}",
                        inputs.len(),
                        limit
                    )));
                }
                let mut handles = Vec::with_capacity(inputs.len());
                for item in inputs {
                    let input = self.coerce_input_for_plan(child_plan, item.clone())?;
                    let child_id = self.start_plan_instance(
                        child_plan,
                        input,
                        parent_context.clone(),
                        None,
                        Some(parent_plan_id),
                    )?;
                    self.scheduler.push_plan(child_id);
                    handles.push(plan_handle_expr_value(child_id, child_plan.clone()));
                }
                if let Some(parent) = self.plan_instances.get_mut(&parent_plan_id) {
                    if parent.deliver_spawn_value(step_id, ExprValue::List(handles)) {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }

    fn handle_plan_wait_request(
        &mut self,
        parent_plan_id: u64,
        request: &crate::plan::PlanWaitRequest,
    ) -> Result<(), KernelError> {
        let completion_map: HashMap<u64, PlanCompletionValue> = self
            .completed_plan_outcomes
            .iter()
            .map(|(id, outcome)| (*id, outcome.await_value.clone()))
            .collect();
        let step_id = match request {
            crate::plan::PlanWaitRequest::AwaitPlan { step_id, .. } => step_id.as_str(),
            crate::plan::PlanWaitRequest::AwaitPlansAll { step_id, .. } => step_id.as_str(),
        };
        if let Some(instance) = self.plan_instances.get_mut(&parent_plan_id) {
            if instance.resolve_plan_waits(&completion_map) {
                self.scheduler.push_plan(parent_plan_id);
                return Ok(());
            }
            for child_id in instance.pending_wait_child_ids(step_id) {
                if self.completed_plan_outcomes.contains_key(&child_id) {
                    continue;
                }
                let watchers = self.plan_wait_watchers.entry(child_id).or_default();
                if !watchers
                    .iter()
                    .any(|watcher| watcher.parent_plan_id == parent_plan_id)
                {
                    watchers.push(PlanWaitWatcher { parent_plan_id });
                }
            }
        }
        Ok(())
    }

    fn wake_plan_waiters(&mut self, completed_plan_id: u64) {
        let Some(watchers) = self.plan_wait_watchers.remove(&completed_plan_id) else {
            return;
        };
        let completion_map: HashMap<u64, PlanCompletionValue> = self
            .completed_plan_outcomes
            .iter()
            .map(|(id, value)| (*id, value.await_value.clone()))
            .collect();
        for watcher in watchers {
            if let Some(parent) = self.plan_instances.get_mut(&watcher.parent_plan_id) {
                if parent.resolve_plan_waits(&completion_map) {
                    self.scheduler.push_plan(watcher.parent_plan_id);
                }
            }
        }
    }

    fn remember_plan_completion(&mut self, plan_id: u64, value: PlanCompletionValue) {
        if self.completed_plan_outcomes.contains_key(&plan_id) {
            return;
        }
        if self.completed_plan_order.len() >= RECENT_PLAN_COMPLETION_CACHE {
            if let Some(oldest) = self.completed_plan_order.pop_front() {
                self.completed_plan_outcomes.remove(&oldest);
            }
        }
        self.completed_plan_order.push_back(plan_id);
        self.completed_plan_outcomes
            .insert(plan_id, PlanCompletionOutcome { await_value: value });
    }

    fn build_plan_completion_value(
        &self,
        outcome: &crate::plan::PlanTickOutcome,
    ) -> PlanCompletionValue {
        if let Some(err) = outcome.plan_error.as_ref() {
            plan_await_variant(
                "Error",
                ExprValue::Record(IndexMap::from([
                    ("code".into(), ExprValue::Text(err.code.clone())),
                    ("message".into(), ExprValue::Text(err.code.clone())),
                ])),
            )
        } else {
            plan_await_variant("Ok", outcome.result.clone().unwrap_or(ExprValue::Unit))
        }
    }
}

fn try_normalize_receipt_payload(
    reducer_event_schema: &TypeExpr,
    schema_index: &SchemaIndex,
    payload_value: CborValue,
    payload_schema: &str,
) -> Result<
    aos_air_types::value_normalize::NormalizedValue,
    aos_air_types::value_normalize::ValueNormalizeError,
> {
    let mut candidates = Vec::new();
    candidates.push(payload_value.clone());
    if let TypeExpr::Variant(variant) = reducer_event_schema {
        for (tag, ty) in &variant.variant {
            if let TypeExpr::Ref(reference) = ty
                && reference.reference.as_str() == payload_schema
            {
                let wrapped = CborValue::Map(BTreeMap::from([
                    (CborValue::Text("$tag".into()), CborValue::Text(tag.clone())),
                    (CborValue::Text("$value".into()), payload_value.clone()),
                ]));
                candidates.push(wrapped);
                break;
            }
        }
    }

    let mut last_err = None;
    for candidate in candidates {
        match normalize_value_with_schema(candidate, reducer_event_schema, schema_index) {
            Ok(normalized) => return Ok(normalized),
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.expect("at least one candidate"))
}

fn plan_handle_expr_value(instance_id: u64, plan: String) -> ExprValue {
    ExprValue::Record(IndexMap::from([
        ("instance_id".into(), ExprValue::Nat(instance_id)),
        ("plan".into(), ExprValue::Text(plan)),
    ]))
}

fn plan_await_variant(tag: &str, value: ExprValue) -> ExprValue {
    ExprValue::Record(IndexMap::from([
        ("$tag".into(), ExprValue::Text(tag.to_string())),
        ("$value".into(), value),
    ]))
}
