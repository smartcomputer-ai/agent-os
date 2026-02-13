use super::*;

impl<S: Store + 'static> Kernel<S> {
    pub fn tick(&mut self) -> Result<(), KernelError> {
        if let Some(task) = self.scheduler.pop() {
            match task {
                Task::Reducer(event) => self.handle_reducer_event(event)?,
                Task::Plan(id) => self.handle_plan_task(id)?,
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

    pub fn drain_effects(&mut self) -> Vec<aos_effects::EffectIntent> {
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
        if let Some(plan_bindings) = self.plan_triggers.get(&event.schema) {
            for binding in plan_bindings {
                if let Some(plan_def) = self.plan_registry.get(&binding.plan) {
                    let normalized = normalize_cbor_by_name(
                        &self.schema_index,
                        event.schema.as_str(),
                        &event.value,
                    )
                    .map_err(|err| {
                        KernelError::Manifest(format!(
                            "failed to decode plan input for {}: {err}",
                            binding.plan
                        ))
                    })?;
                    let input_schema =
                        self.schema_index
                            .get(plan_def.input.as_str())
                            .ok_or_else(|| {
                                KernelError::Manifest(format!(
                                    "plan '{}' input schema '{}' not found",
                                    plan_def.name, plan_def.input
                                ))
                            })?;
                    let input =
                        cbor_to_expr_value(&normalized.value, input_schema, &self.schema_index)?;
                    let correlation =
                        determine_correlation_value(binding, &input, event.key.as_ref());
                    let instance_id = self.scheduler.alloc_plan_id();
                    let cap_handles = self
                        .plan_cap_handles
                        .get(&plan_def.name)
                        .ok_or_else(|| {
                            KernelError::Manifest(format!(
                                "plan '{}' missing cap bindings",
                                plan_def.name
                            ))
                        })?
                        .clone();
                    let mut instance = PlanInstance::new(
                        instance_id,
                        plan_def.clone(),
                        input,
                        self.schema_index.clone(),
                        correlation,
                        cap_handles,
                    );
                    instance.set_context(crate::plan::PlanContext::from_stamp(stamp));
                    self.plan_instances.insert(instance_id, instance);
                    self.scheduler.push_plan(instance_id);
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
            let (plan_name, outcome, step_states) = {
                let instance = self
                    .plan_instances
                    .get_mut(&id)
                    .expect("instance must exist");
                let name = instance.name.clone();
                let snapshot = instance.snapshot();
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
                (name, outcome, snapshot.step_states)
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
            if let Some(schema) = outcome.waiting_event.clone() {
                self.waiting_events.entry(schema).or_default().push(id);
            }
            if outcome.completed {
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
            } else if outcome.waiting_receipts.is_empty() && outcome.waiting_event.is_none() {
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
        if let Some(pending) = self.pending_receipts.remove(&receipt.intent_hash) {
            self.record_effect_receipt(&receipt, &stamp)?;
            self.record_decisions()?;
            if let Some(instance) = self.plan_instances.get_mut(&pending.plan_id) {
                instance.set_context(crate::plan::PlanContext::from_stamp(&stamp));
                if instance.deliver_receipt(receipt.intent_hash, &receipt.payload_cbor)? {
                    self.scheduler.push_plan(pending.plan_id);
                }
                self.remember_receipt(receipt.intent_hash);
                return Ok(());
            } else {
                log::warn!(
                    "receipt {} arrived for completed plan {}",
                    format_intent_hash(&receipt.intent_hash),
                    pending.plan_id
                );
                self.remember_receipt(receipt.intent_hash);
                return Ok(());
            }
        }

        if self.recent_receipt_index.contains(&receipt.intent_hash) {
            log::warn!(
                "late receipt {} ignored (already applied)",
                format_intent_hash(&receipt.intent_hash)
            );
            return Ok(());
        }

        if let Some(context) = self.pending_reducer_receipts.remove(&receipt.intent_hash) {
            self.record_effect_receipt(&receipt, &stamp)?;
            self.record_decisions()?;
            let event = build_reducer_receipt_event(&context, &receipt)?;
            self.process_domain_event(event)?;
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

    fn record_plan_ended(&mut self, record: PlanEndedRecord) -> Result<(), KernelError> {
        self.append_record(JournalRecord::PlanEnded(record))
    }
}
