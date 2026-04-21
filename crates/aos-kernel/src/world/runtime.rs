use super::*;
use serde::Serialize;
use std::collections::HashSet;

impl<S: Store + 'static> Kernel<S> {
    pub fn submit_domain_event(
        &mut self,
        schema: String,
        value: Vec<u8>,
    ) -> Result<(), KernelError> {
        self.submit_domain_event_result(schema, value)
    }

    pub fn submit_domain_event_result(
        &mut self,
        schema: impl Into<String>,
        value: Vec<u8>,
    ) -> Result<(), KernelError> {
        self.accept(WorldInput::DomainEvent(DomainEvent {
            schema: schema.into(),
            value,
            key: None,
        }))
    }

    /// Compatibility helper for scripted harnesses that expect a mutable effect outbox.
    pub fn drain_effects(&mut self) -> Result<Vec<aos_effects::EffectIntent>, KernelError> {
        self.tick_until_idle()?;

        let mut intents = self.effect_manager.drain()?;
        let mut seen: HashSet<[u8; 32]> = intents.iter().map(|intent| intent.intent_hash).collect();

        let drain = self.drain_until_idle_from(self.compat_drain_cursor)?;
        self.compat_drain_cursor = drain.tail.to;
        intents.extend(
            drain
                .opened_effects
                .into_iter()
                .map(|opened| opened.intent)
                .filter(|intent| {
                    self.pending_workflow_receipts
                        .contains_key(&intent.intent_hash)
                })
                .filter(|intent| seen.insert(intent.intent_hash)),
        );
        Ok(intents)
    }

    pub fn queued_effects_snapshot(&self) -> Vec<aos_effects::EffectIntent> {
        let mut intents = self.effect_manager.queued().to_vec();
        let mut seen: HashSet<[u8; 32]> = intents.iter().map(|intent| intent.intent_hash).collect();
        intents.extend(
            self.snapshot_queued_effects()
                .into_iter()
                .map(|snapshot| snapshot.into_intent())
                .filter(|intent| seen.insert(intent.intent_hash)),
        );
        intents
    }

    pub fn accept(&mut self, input: WorldInput) -> Result<(), KernelError> {
        match input {
            WorldInput::DomainEvent(event) => self.process_domain_event(event),
            WorldInput::Receipt(receipt) => self.handle_receipt(receipt),
            WorldInput::StreamFrame(frame) => self.handle_stream_frame(frame),
        }
    }

    pub fn apply_control(
        &mut self,
        control: WorldControl,
    ) -> Result<WorldControlOutcome, KernelError> {
        match control {
            WorldControl::SubmitProposal { patch, description } => {
                let proposal_id = self.submit_proposal(patch, description)?;
                Ok(WorldControlOutcome::ProposalSubmitted { proposal_id })
            }
            WorldControl::RunShadow { proposal_id } => {
                let _ = self.run_shadow(proposal_id, None)?;
                Ok(WorldControlOutcome::ShadowRun { proposal_id })
            }
            WorldControl::DecideProposal {
                proposal_id,
                approver,
                decision,
            } => {
                match decision {
                    ApprovalDecisionRecord::Approve => {
                        self.approve_proposal(proposal_id, approver)?
                    }
                    ApprovalDecisionRecord::Reject => {
                        self.reject_proposal(proposal_id, approver)?
                    }
                }
                Ok(WorldControlOutcome::ProposalDecided {
                    proposal_id,
                    decision,
                })
            }
            WorldControl::ApplyProposal { proposal_id } => {
                self.apply_proposal(proposal_id)?;
                Ok(WorldControlOutcome::ProposalApplied { proposal_id })
            }
            WorldControl::ApplyPatchDirect { patch } => {
                let manifest_hash = self.apply_patch_direct(patch)?;
                Ok(WorldControlOutcome::PatchApplied { manifest_hash })
            }
        }
    }

    pub(crate) fn tick(&mut self) -> Result<(), KernelError> {
        if let Some(event) = self.workflow_queue.pop_front() {
            self.handle_workflow_event(event)?;
        }
        Ok(())
    }

    pub fn tick_until_idle(&mut self) -> Result<(), KernelError> {
        while !self.workflow_queue.is_empty() {
            self.tick()?;
        }
        Ok(())
    }

    pub fn drain_until_idle_from(
        &mut self,
        tail_start: JournalSeq,
    ) -> Result<KernelDrain, KernelError> {
        self.tick_until_idle()?;
        let tail = self.tail_scan_from(tail_start)?;
        let opened_effects = self.collect_opened_effects_from_tail(&tail)?;
        let quiescence = self.quiescence_status();
        Ok(KernelDrain {
            tail_start,
            tail,
            opened_effects,
            kernel_idle: quiescence.kernel_idle,
            quiescence,
        })
    }

    fn materialize_effect_record(
        &mut self,
        record: &EffectIntentRecord,
    ) -> Result<aos_effects::EffectIntent, KernelError> {
        let params_cbor = self.hydrate_externalized_cbor(
            record.params_cbor.clone(),
            record.params_ref.as_ref(),
            record.params_size,
            record.params_sha256.as_ref(),
            "effect_intent.params",
        )?;
        let mut intent = aos_effects::EffectIntent::from_raw_params(
            aos_effects::EffectKind::new(record.kind.clone()),
            params_cbor,
            record.idempotency_key,
        )
        .map_err(|err| KernelError::EffectManager(err.to_string()))?;
        self.effect_manager
            .prepare_intent_for_execution(&mut intent)?;
        Ok(intent)
    }

    fn collect_opened_effects_from_tail(
        &mut self,
        tail: &TailScan,
    ) -> Result<Vec<OpenedEffect>, KernelError> {
        tail.intents
            .iter()
            .map(|opened| {
                Ok(OpenedEffect {
                    seq: opened.seq,
                    record: opened.record.clone(),
                    intent: self.materialize_effect_record(&opened.record)?,
                })
            })
            .collect()
    }

    pub fn handle_receipt(
        &mut self,
        receipt: aos_effects::EffectReceipt,
    ) -> Result<(), KernelError> {
        let journal_height = self.journal.next_seq();
        let stamp = self.sample_ingress(journal_height)?;
        self.handle_receipt_with_ingress(receipt, stamp)
    }

    fn handle_stream_frame(
        &mut self,
        frame: aos_effects::EffectStreamFrame,
    ) -> Result<(), KernelError> {
        let journal_height = self.journal.next_seq();
        let stamp = self.sample_ingress(journal_height)?;
        self.handle_stream_frame_with_ingress(frame, stamp)
    }

    pub(super) fn handle_receipt_with_ingress(
        &mut self,
        receipt: aos_effects::EffectReceipt,
        stamp: IngressStamp,
    ) -> Result<(), KernelError> {
        Self::validate_entropy(&stamp.entropy)?;

        if self.recent_receipt_index.contains(&receipt.intent_hash) {
            if let Some(context) = self
                .pending_workflow_receipts
                .get(&receipt.intent_hash)
                .cloned()
            {
                self.settle_workflow_receipt_intent(&context, receipt.intent_hash);
            }
            log::trace!(
                "late receipt {} ignored (already applied)",
                format_intent_hash(&receipt.intent_hash)
            );
            return Ok(());
        }

        if let Some(context) = self
            .pending_workflow_receipts
            .get(&receipt.intent_hash)
            .cloned()
        {
            if self.suppress_journal {
                self.deliver_receipt_to_workflow_instance(&context, &receipt, &stamp)?;
                self.settle_workflow_receipt_intent(&context, receipt.intent_hash);
                return Ok(());
            }
            enum ReceiptFaultKind {
                InvalidPayload,
                DeliveryFailed,
            }

            let processed = match self
                .normalize_receipt_payload_for_effect(receipt.clone(), &context)
            {
                Ok(normalized_receipt) => {
                    self.record_effect_receipt(&normalized_receipt, &stamp)?;
                    self.record_decisions()?;
                    self.deliver_receipt_to_workflow_instance(&context, &normalized_receipt, &stamp)
                        .map(|()| normalized_receipt)
                        .map_err(|err| (ReceiptFaultKind::DeliveryFailed, err))
                }
                Err(err) => {
                    self.record_effect_receipt(&receipt, &stamp)?;
                    self.record_decisions()?;
                    Err((ReceiptFaultKind::InvalidPayload, err))
                }
            };

            match processed {
                Ok(normalized_receipt) => {
                    self.settle_workflow_receipt_intent(&context, normalized_receipt.intent_hash);
                }
                Err((kind, err)) => {
                    let error_code = match kind {
                        ReceiptFaultKind::InvalidPayload => "receipt.invalid_payload",
                        ReceiptFaultKind::DeliveryFailed => "receipt.delivery_failed",
                    };
                    self.handle_workflow_receipt_fault(
                        &context,
                        &receipt,
                        &stamp,
                        error_code,
                        err.to_string(),
                    )?;
                }
            }
            return Ok(());
        }

        if self.suppress_journal {
            log::trace!(
                "receipt {} ignored during replay (no pending context)",
                format_intent_hash(&receipt.intent_hash)
            );
            return Ok(());
        }

        Err(KernelError::UnknownReceipt(format_intent_hash(
            &receipt.intent_hash,
        )))
    }

    pub(super) fn handle_stream_frame_with_ingress(
        &mut self,
        frame: aos_effects::EffectStreamFrame,
        stamp: IngressStamp,
    ) -> Result<(), KernelError> {
        Self::validate_entropy(&stamp.entropy)?;

        let Some(context) = self
            .pending_workflow_receipts
            .get(&frame.intent_hash)
            .cloned()
        else {
            self.record_workflow_stream_drop(
                &frame,
                "stream.unknown_intent",
                "no in-flight intent context for frame",
            )?;
            return Ok(());
        };

        if frame.origin_module_id != context.origin_module_id
            || frame.origin_instance_key != context.origin_instance_key
            || frame.effect_kind != context.effect_kind
            || frame.emitted_at_seq != context.emitted_at_seq
        {
            self.record_workflow_stream_drop(
                &frame,
                "stream.identity_mismatch",
                "frame identity does not match recorded in-flight intent",
            )?;
            return Ok(());
        }

        if self.suppress_journal {
            self.deliver_stream_frame_to_workflow_instance(&context, &frame, &stamp)?;
            let advanced = self.advance_workflow_stream_cursor(
                &context.origin_module_id,
                context.origin_instance_key.as_deref(),
                frame.intent_hash,
                frame.seq,
            );
            if !advanced {
                self.record_workflow_stream_drop(
                    &frame,
                    "stream.cursor_unavailable",
                    "stream cursor missing for in-flight intent",
                )?;
            }
            return Ok(());
        }

        let last_seq = self
            .workflow_stream_cursor(
                &context.origin_module_id,
                context.origin_instance_key.as_deref(),
                frame.intent_hash,
            )
            .unwrap_or_else(|| {
                self.record_workflow_inflight_intent(
                    &context.origin_module_id,
                    context.origin_instance_key.as_deref(),
                    frame.intent_hash,
                    &context.effect_kind,
                    &context.params_cbor,
                    context.emitted_at_seq,
                );
                0
            });

        if frame.seq <= last_seq {
            self.record_workflow_stream_drop(
                &frame,
                "stream.non_monotonic",
                "frame seq is duplicate or out-of-order",
            )?;
            return Ok(());
        }

        if frame.seq > last_seq.saturating_add(1) {
            self.record_workflow_stream_gap(&frame, last_seq.saturating_add(1))?;
        }

        self.record_stream_frame(&frame, &stamp)?;
        self.record_decisions()?;
        self.deliver_stream_frame_to_workflow_instance(&context, &frame, &stamp)?;
        let advanced = self.advance_workflow_stream_cursor(
            &context.origin_module_id,
            context.origin_instance_key.as_deref(),
            frame.intent_hash,
            frame.seq,
        );
        if !advanced {
            self.record_workflow_stream_drop(
                &frame,
                "stream.cursor_unavailable",
                "stream cursor missing for in-flight intent",
            )?;
        }
        Ok(())
    }

    fn normalize_receipt_payload_for_effect(
        &self,
        mut receipt: aos_effects::EffectReceipt,
        context: &WorkflowEffectContext,
    ) -> Result<aos_effects::EffectReceipt, KernelError> {
        let receipt_schema = self
            .effect_defs
            .values()
            .find(|def| def.kind.as_str() == context.effect_kind)
            .map(|def| def.receipt_schema.as_str().to_string())
            .ok_or_else(|| KernelError::UnsupportedEffectKind(context.effect_kind.clone()))?;
        let normalized =
            normalize_cbor_by_name(&self.schema_index, &receipt_schema, &receipt.payload_cbor)
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
        context: &WorkflowEffectContext,
        receipt: &aos_effects::EffectReceipt,
        stamp: &IngressStamp,
    ) -> Result<(), KernelError> {
        let workflow_name = context.origin_module_id.clone();
        let (workflow_event_schema_name, workflow_event_schema) =
            self.resolve_workflow_event_schema(&workflow_name)?;

        let generic_event = crate::receipts::build_workflow_receipt_event(context, receipt)?;
        let generic_value: CborValue = serde_cbor::from_slice(&generic_event.value)
            .map_err(|err| KernelError::ReceiptDecode(err.to_string()))?;
        let normalized = match try_normalize_receipt_payload(
            workflow_event_schema,
            &self.schema_index,
            generic_value.clone(),
            crate::receipts::SYS_EFFECT_RECEIPT_ENVELOPE_SCHEMA,
        ) {
            Ok(normalized) => normalized,
            Err(generic_err) => {
                if let Some(legacy_event) =
                    crate::receipts::build_legacy_workflow_receipt_event(context, receipt)?
                {
                    let legacy_value: CborValue = serde_cbor::from_slice(&legacy_event.value)
                        .map_err(|err| KernelError::ReceiptDecode(err.to_string()))?;
                    try_normalize_receipt_payload(
                        workflow_event_schema,
                        &self.schema_index,
                        legacy_value,
                        legacy_event.schema.as_str(),
                    )
                    .map_err(|legacy_err| {
                        KernelError::Manifest(format!(
                            "receipt payload for '{workflow_name}' does not match event schema '{}': generic={generic_err}; legacy={legacy_err}",
                            workflow_event_schema_name
                        ))
                    })?
                } else {
                    return Err(KernelError::Manifest(format!(
                        "receipt payload for '{workflow_name}' does not match event schema '{}': {generic_err}",
                        workflow_event_schema_name
                    )));
                }
            }
        };

        let mut event = DomainEvent::new(workflow_event_schema_name, normalized.bytes);
        event.key = context.origin_instance_key.clone();
        self.workflow_queue.push_back(WorkflowEvent {
            workflow: workflow_name.clone(),
            event,
            stamp: stamp.clone(),
        });
        Ok(())
    }

    fn deliver_stream_frame_to_workflow_instance(
        &mut self,
        context: &WorkflowEffectContext,
        frame: &aos_effects::EffectStreamFrame,
        stamp: &IngressStamp,
    ) -> Result<(), KernelError> {
        let workflow_name = context.origin_module_id.clone();
        let (workflow_event_schema_name, workflow_event_schema) =
            self.resolve_workflow_event_schema(&workflow_name)?;

        let stream_event = crate::receipts::build_workflow_stream_frame_event(context, frame)?;
        let stream_value: CborValue = serde_cbor::from_slice(&stream_event.value)
            .map_err(|err| KernelError::ReceiptDecode(err.to_string()))?;
        let normalized = try_normalize_receipt_payload(
            workflow_event_schema,
            &self.schema_index,
            stream_value,
            crate::receipts::SYS_EFFECT_STREAM_FRAME_SCHEMA,
        )
        .map_err(|err| {
            KernelError::Manifest(format!(
                "stream frame payload for '{workflow_name}' does not match event schema '{}': {err}",
                workflow_event_schema_name
            ))
        })?;

        let mut event = DomainEvent::new(workflow_event_schema_name, normalized.bytes);
        event.key = context.origin_instance_key.clone();
        self.workflow_queue.push_back(WorkflowEvent {
            workflow: workflow_name,
            event,
            stamp: stamp.clone(),
        });
        Ok(())
    }

    fn resolve_workflow_event_schema<'a>(
        &'a self,
        workflow_name: &str,
    ) -> Result<(String, &'a TypeExpr), KernelError> {
        let module_def = self
            .module_defs
            .get(workflow_name)
            .ok_or_else(|| KernelError::WorkflowNotFound(workflow_name.to_string()))?;
        let workflow_abi = module_def.abi.workflow.as_ref().ok_or_else(|| {
            KernelError::Manifest(format!(
                "module '{workflow_name}' is not a workflow/workflow"
            ))
        })?;
        let workflow_event_schema_name = workflow_abi.event.as_str().to_string();
        let workflow_event_schema = self
            .schema_index
            .get(workflow_event_schema_name.as_str())
            .ok_or_else(|| {
                KernelError::Manifest(format!(
                    "schema '{}' not found for workflow module '{}'",
                    workflow_event_schema_name, workflow_name
                ))
            })?;
        Ok((workflow_event_schema_name, workflow_event_schema))
    }

    pub(crate) fn settle_workflow_receipt_intent(
        &mut self,
        context: &WorkflowEffectContext,
        intent_hash: [u8; 32],
    ) {
        self.pending_workflow_receipts.remove(&intent_hash);
        self.mark_workflow_receipt_settled(
            &context.origin_module_id,
            context.origin_instance_key.as_deref(),
            intent_hash,
        );
        self.remember_receipt(intent_hash);
    }

    fn fail_workflow_instance(
        &mut self,
        context: &WorkflowEffectContext,
        last_processed_event_seq: u64,
    ) {
        let instance_id = workflow_instance_id(
            context.origin_module_id.as_str(),
            context.origin_instance_key.as_deref(),
        );
        let entry = self
            .workflow_instances
            .entry(instance_id)
            .or_insert_with(|| WorkflowInstanceState {
                state_bytes: Vec::new(),
                inflight_intents: std::collections::BTreeMap::new(),
                status: WorkflowRuntimeStatus::Failed,
                last_processed_event_seq,
                module_version: context.module_version.clone(),
            });
        entry.inflight_intents.clear();
        entry.status = WorkflowRuntimeStatus::Failed;
        entry.last_processed_event_seq =
            entry.last_processed_event_seq.max(last_processed_event_seq);
        if entry.module_version.is_none() {
            entry.module_version = context.module_version.clone();
        }
    }

    fn try_deliver_workflow_rejected_receipt_event(
        &mut self,
        context: &WorkflowEffectContext,
        receipt: &aos_effects::EffectReceipt,
        stamp: &IngressStamp,
        error_code: &str,
        error_message: &str,
    ) -> Result<bool, KernelError> {
        let workflow_name = context.origin_module_id.clone();
        let (workflow_event_schema_name, workflow_event_schema) =
            self.resolve_workflow_event_schema(&workflow_name)?;
        let rejected_event = crate::receipts::build_workflow_receipt_rejected_event(
            context,
            receipt,
            error_code,
            error_message,
        )?;
        let rejected_value: CborValue = serde_cbor::from_slice(&rejected_event.value)
            .map_err(|err| KernelError::ReceiptDecode(err.to_string()))?;
        let Ok(normalized) = try_normalize_receipt_payload(
            workflow_event_schema,
            &self.schema_index,
            rejected_value,
            crate::receipts::SYS_EFFECT_RECEIPT_REJECTED_SCHEMA,
        ) else {
            return Ok(false);
        };

        let mut event = DomainEvent::new(workflow_event_schema_name, normalized.bytes);
        event.key = context.origin_instance_key.clone();
        self.workflow_queue.push_back(WorkflowEvent {
            workflow: workflow_name,
            event,
            stamp: stamp.clone(),
        });
        Ok(true)
    }

    fn handle_workflow_receipt_fault(
        &mut self,
        context: &WorkflowEffectContext,
        receipt: &aos_effects::EffectReceipt,
        stamp: &IngressStamp,
        error_code: &str,
        error_message: String,
    ) -> Result<(), KernelError> {
        self.settle_workflow_receipt_intent(context, receipt.intent_hash);

        let delivered_rejected = match self.try_deliver_workflow_rejected_receipt_event(
            context,
            receipt,
            stamp,
            error_code,
            error_message.as_str(),
        ) {
            Ok(delivered) => delivered,
            Err(err) => {
                log::warn!(
                    "failed to deliver rejected receipt event for '{}': {}",
                    context.origin_module_id,
                    err
                );
                false
            }
        };

        let (workflow_failed, dropped_pending_receipts) = if delivered_rejected {
            (false, 0usize)
        } else {
            let dropped = self.clear_pending_receipts_for_workflow_instance(
                &context.origin_module_id,
                context.origin_instance_key.as_deref(),
            );
            self.fail_workflow_instance(context, stamp.journal_height);
            (true, dropped)
        };

        self.record_workflow_receipt_fault(
            context,
            receipt,
            error_code,
            error_message.as_str(),
            delivered_rejected,
            workflow_failed,
            dropped_pending_receipts,
        )
    }

    fn record_workflow_receipt_fault(
        &mut self,
        context: &WorkflowEffectContext,
        receipt: &aos_effects::EffectReceipt,
        error_code: &str,
        error_message: &str,
        delivered_rejected: bool,
        workflow_failed: bool,
        dropped_pending_receipts: usize,
    ) -> Result<(), KernelError> {
        let payload = WorkflowReceiptFaultRecord {
            origin_module_id: context.origin_module_id.clone(),
            origin_instance_key: context.origin_instance_key.clone(),
            intent_id: format_intent_hash(&receipt.intent_hash),
            effect_kind: context.effect_kind.clone(),
            adapter_id: receipt.adapter_id.clone(),
            status: receipt.status.clone(),
            error_code: error_code.to_string(),
            error_message: error_message.to_string(),
            delivered_rejected,
            workflow_failed,
            dropped_pending_receipts: dropped_pending_receipts as u64,
        };
        let data =
            serde_cbor::to_vec(&payload).map_err(|err| KernelError::Journal(err.to_string()))?;
        self.append_record(JournalRecord::Custom(crate::journal::CustomRecord {
            tag: "workflow.receipt_fault".to_string(),
            data,
        }))
    }

    fn record_workflow_stream_gap(
        &mut self,
        frame: &aos_effects::EffectStreamFrame,
        expected_seq: u64,
    ) -> Result<(), KernelError> {
        let payload = WorkflowStreamGapRecord {
            origin_module_id: frame.origin_module_id.clone(),
            origin_instance_key: frame.origin_instance_key.clone(),
            intent_id: format_intent_hash(&frame.intent_hash),
            effect_kind: frame.effect_kind.clone(),
            adapter_id: frame.adapter_id.clone(),
            expected_seq,
            observed_seq: frame.seq,
        };
        let data =
            serde_cbor::to_vec(&payload).map_err(|err| KernelError::Journal(err.to_string()))?;
        self.append_record(JournalRecord::Custom(crate::journal::CustomRecord {
            tag: "workflow.stream_gap".to_string(),
            data,
        }))
    }

    fn record_workflow_stream_drop(
        &mut self,
        frame: &aos_effects::EffectStreamFrame,
        reason_code: &str,
        reason_message: &str,
    ) -> Result<(), KernelError> {
        let payload = WorkflowStreamDropRecord {
            origin_module_id: frame.origin_module_id.clone(),
            origin_instance_key: frame.origin_instance_key.clone(),
            intent_id: format_intent_hash(&frame.intent_hash),
            effect_kind: frame.effect_kind.clone(),
            adapter_id: frame.adapter_id.clone(),
            emitted_at_seq: frame.emitted_at_seq,
            seq: frame.seq,
            kind: frame.kind.clone(),
            reason_code: reason_code.to_string(),
            reason_message: reason_message.to_string(),
        };
        let data =
            serde_cbor::to_vec(&payload).map_err(|err| KernelError::Journal(err.to_string()))?;
        self.append_record(JournalRecord::Custom(crate::journal::CustomRecord {
            tag: "workflow.stream_drop".to_string(),
            data,
        }))
    }

    fn remember_receipt(&mut self, hash: [u8; 32]) {
        if self.recent_receipt_index.contains(&hash) {
            return;
        }
        if self.recent_receipts.len() >= RECENT_RECEIPT_CACHE
            && let Some(old) = self.recent_receipts.pop_front()
        {
            self.recent_receipt_index.remove(&old);
        }
        self.recent_receipts.push_back(hash);
        self.recent_receipt_index.insert(hash);
    }
}

fn try_normalize_receipt_payload(
    workflow_event_schema: &TypeExpr,
    schema_index: &SchemaIndex,
    payload_value: CborValue,
    payload_schema: &str,
) -> Result<
    aos_air_types::value_normalize::NormalizedValue,
    aos_air_types::value_normalize::ValueNormalizeError,
> {
    let mut candidates = Vec::new();
    candidates.push(payload_value.clone());
    if let TypeExpr::Variant(variant) = workflow_event_schema {
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
        match aos_air_types::value_normalize::normalize_value_with_schema(
            candidate,
            workflow_event_schema,
            schema_index,
        ) {
            Ok(normalized) => return Ok(normalized),
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.expect("at least one candidate"))
}

#[derive(Serialize)]
struct WorkflowReceiptFaultRecord {
    origin_module_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    origin_instance_key: Option<Vec<u8>>,
    intent_id: String,
    effect_kind: String,
    adapter_id: String,
    status: aos_effects::ReceiptStatus,
    error_code: String,
    error_message: String,
    delivered_rejected: bool,
    workflow_failed: bool,
    dropped_pending_receipts: u64,
}

#[derive(Serialize)]
struct WorkflowStreamGapRecord {
    origin_module_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    origin_instance_key: Option<Vec<u8>>,
    intent_id: String,
    effect_kind: String,
    adapter_id: String,
    expected_seq: u64,
    observed_seq: u64,
}

#[derive(Serialize)]
struct WorkflowStreamDropRecord {
    origin_module_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    origin_instance_key: Option<Vec<u8>>,
    intent_id: String,
    effect_kind: String,
    adapter_id: String,
    emitted_at_seq: u64,
    seq: u64,
    kind: String,
    reason_code: String,
    reason_message: String,
}

mod serde_bytes_opt {
    use serde::{Deserialize, Deserializer, Serializer};
    use serde_bytes::{ByteBuf, Bytes};

    pub fn serialize<S>(value: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(bytes) => serializer.serialize_some(Bytes::new(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<ByteBuf>::deserialize(deserializer).map(|opt| opt.map(|buf| buf.into_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemStore;
    use crate::governance::ManifestPatch;
    use crate::journal::Journal;
    use crate::journal::JournalKind;
    use crate::world::test_support::{hash, minimal_manifest, schema_event_record, schema_text};
    use aos_air_types::{
        DefModule, HashRef, ModuleAbi, ModuleKind, NamedRef, SchemaRef, WorkflowAbi, builtins,
        catalog::EffectCatalog,
    };
    use aos_effects::{EffectStreamFrame, ReceiptStatus, builtins::TimerSetParams};
    use aos_wasm_abi::WorkflowEffect;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    fn install_stream_module(kernel: &mut Kernel<MemStore>, module_name: &str) {
        let module = aos_air_types::DefModule {
            name: module_name.into(),
            module_kind: ModuleKind::Workflow,
            wasm_hash: HashRef::new(format!("sha256:{}", "1".repeat(64))).unwrap(),
            key_schema: None,
            abi: ModuleAbi {
                workflow: Some(WorkflowAbi {
                    state: SchemaRef::new("sys/PendingEffect@1").unwrap(),
                    event: SchemaRef::new(crate::receipts::SYS_EFFECT_STREAM_FRAME_SCHEMA).unwrap(),
                    context: None,
                    annotations: None,
                    effects_emitted: vec![],
                }),
                pure: None,
            },
        };
        kernel.module_defs.insert(module_name.into(), module);
    }

    fn kernel_with_stream_context(
        module_name: &str,
        intent_hash: [u8; 32],
        emitted_at_seq: u64,
    ) -> Kernel<MemStore> {
        let store = Arc::new(MemStore::default());
        let journal = Journal::new();
        let mut kernel = crate::world::test_support::kernel_with_store_and_journal(store, journal);
        install_stream_module(&mut kernel, module_name);
        let context = WorkflowEffectContext::new(
            module_name.into(),
            None,
            "http.request".into(),
            vec![1, 2, 3],
            [0x33u8; 32],
            None,
            intent_hash,
            emitted_at_seq,
            None,
        );
        kernel
            .pending_workflow_receipts
            .insert(intent_hash, context.clone());
        kernel.record_workflow_inflight_intent(
            module_name,
            None,
            intent_hash,
            &context.effect_kind,
            &context.params_cbor,
            emitted_at_seq,
        );
        kernel
    }

    fn kernel_for_timer_effects() -> Kernel<MemStore> {
        let store = Arc::new(MemStore::default());
        let workflow = "com.acme/Workflow@1";
        let module = DefModule {
            name: workflow.into(),
            module_kind: ModuleKind::Workflow,
            wasm_hash: HashRef::new(format!("sha256:{}", "1".repeat(64))).unwrap(),
            key_schema: None,
            abi: ModuleAbi {
                workflow: Some(WorkflowAbi {
                    state: SchemaRef::new("com.acme/State@1").unwrap(),
                    event: SchemaRef::new("com.acme/Event@1").unwrap(),
                    context: None,
                    annotations: None,
                    effects_emitted: vec![aos_air_types::EffectKind::timer_set()],
                }),
                pure: None,
            },
        };
        let timer_effect = builtins::find_builtin_effect("sys/timer.set@1")
            .expect("builtin timer effect")
            .effect
            .clone();

        let mut manifest = minimal_manifest();
        manifest.modules = vec![NamedRef {
            name: workflow.into(),
            hash: HashRef::new(hash(1)).unwrap(),
        }];
        manifest.effects = vec![NamedRef {
            name: timer_effect.name.clone(),
            hash: HashRef::new(hash(2)).unwrap(),
        }];

        let loaded = LoadedManifest {
            manifest,
            secrets: vec![],
            modules: HashMap::from([(module.name.clone(), module)]),
            effects: HashMap::from([(timer_effect.name.clone(), timer_effect.clone())]),
            schemas: HashMap::from([
                ("com.acme/State@1".into(), schema_text("com.acme/State@1")),
                (
                    "com.acme/Event@1".into(),
                    schema_event_record("com.acme/Event@1"),
                ),
            ]),
            effect_catalog: EffectCatalog::from_defs(vec![timer_effect]),
        };

        Kernel::from_loaded_manifest(store, loaded, Journal::new()).expect("timer kernel")
    }

    fn emit_timer_effect(kernel: &mut Kernel<MemStore>, deliver_at_ns: u64) {
        let params = TimerSetParams {
            deliver_at_ns,
            key: Some(format!("t-{deliver_at_ns}")),
        };
        kernel
            .handle_workflow_output(
                "com.acme/Workflow@1".into(),
                None,
                false,
                WorkflowOutput {
                    effects: vec![WorkflowEffect {
                        kind: aos_effects::EffectKind::TIMER_SET.into(),
                        params_cbor: serde_cbor::to_vec(&params).expect("encode timer params"),
                        cap_slot: None,
                        issuer_ref: None,
                        idempotency_key: None,
                    }],
                    ..Default::default()
                },
            )
            .expect("emit timer effect");
    }

    #[test]
    fn stream_frame_routes_and_advances_cursor() {
        let intent_hash = [7u8; 32];
        let mut kernel = kernel_with_stream_context("com.acme/Workflow@1", intent_hash, 11);
        let frame = EffectStreamFrame {
            intent_hash,
            adapter_id: "adapter.stream".into(),
            origin_module_id: "com.acme/Workflow@1".into(),
            origin_instance_key: None,
            effect_kind: "http.request".into(),
            emitted_at_seq: 11,
            seq: 1,
            kind: "progress".into(),
            payload_cbor: vec![0xA1],
            payload_ref: None,
            signature: vec![],
        };
        kernel
            .accept(WorldInput::StreamFrame(frame.clone()))
            .expect("accept stream");
        assert_eq!(kernel.workflow_queue.len(), 1);
        let instances = kernel.workflow_instances_snapshot();
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].inflight_intents[0].last_stream_seq, 1);
        let entries = kernel.dump_journal().expect("journal");
        assert!(
            entries
                .iter()
                .any(|entry| entry.kind == JournalKind::StreamFrame),
            "expected stream frame record in journal"
        );

        kernel.workflow_queue.clear();
        kernel
            .accept(WorldInput::StreamFrame(frame))
            .expect("duplicate frame should be dropped");
        assert!(
            kernel.workflow_queue.is_empty(),
            "duplicate frame must not enqueue workflow event"
        );
        let instances = kernel.workflow_instances_snapshot();
        assert_eq!(instances[0].inflight_intents[0].last_stream_seq, 1);
    }

    #[test]
    fn stream_frame_gap_is_accepted_and_logged() {
        let intent_hash = [8u8; 32];
        let mut kernel = kernel_with_stream_context("com.acme/Workflow@1", intent_hash, 12);
        let frame = EffectStreamFrame {
            intent_hash,
            adapter_id: "adapter.stream".into(),
            origin_module_id: "com.acme/Workflow@1".into(),
            origin_instance_key: None,
            effect_kind: "http.request".into(),
            emitted_at_seq: 12,
            seq: 3,
            kind: "progress".into(),
            payload_cbor: vec![0xA2],
            payload_ref: None,
            signature: vec![],
        };
        kernel
            .accept(WorldInput::StreamFrame(frame))
            .expect("accept stream gap");
        let entries = kernel.dump_journal().expect("journal");
        let mut has_gap = false;
        for entry in entries {
            if entry.kind != JournalKind::Custom {
                continue;
            }
            let record: crate::journal::JournalRecord =
                serde_cbor::from_slice(&entry.payload).expect("decode custom record");
            if let crate::journal::JournalRecord::Custom(custom) = record
                && custom.tag == "workflow.stream_gap"
            {
                has_gap = true;
            }
        }
        assert!(has_gap, "expected gap diagnostic record");
        let instances = kernel.workflow_instances_snapshot();
        assert_eq!(instances[0].inflight_intents[0].last_stream_seq, 3);
    }

    #[test]
    fn duplicate_receipt_settles_stale_pending_context() {
        let intent_hash = [0x55u8; 32];
        let mut kernel = kernel_with_stream_context("com.acme/Workflow@1", intent_hash, 11);
        kernel.recent_receipts.push_back(intent_hash);
        kernel.recent_receipt_index.insert(intent_hash);

        let receipt = EffectReceipt {
            intent_hash,
            adapter_id: "adapter.stream".into(),
            status: ReceiptStatus::Ok,
            payload_cbor: vec![],
            cost_cents: None,
            signature: vec![],
        };

        kernel
            .accept(WorldInput::Receipt(receipt))
            .expect("duplicate receipt should be ignored and settle stale context");

        assert!(
            kernel.pending_workflow_receipts_snapshot().is_empty(),
            "duplicate receipt should clear stale pending receipt state"
        );
        assert_eq!(
            kernel.workflow_instances_snapshot()[0]
                .inflight_intents
                .len(),
            0,
            "duplicate receipt should clear stale inflight intent"
        );
    }

    #[test]
    fn drain_until_idle_from_returns_only_newly_opened_effects_from_tail_slice() {
        let mut kernel = kernel_for_timer_effects();

        emit_timer_effect(&mut kernel, 10);
        let tail_start = kernel.journal_head();
        emit_timer_effect(&mut kernel, 20);

        let drain = kernel
            .drain_until_idle_from(tail_start)
            .expect("drain from recorded tail");

        assert!(drain.kernel_idle, "expected empty workflow queue");
        assert_eq!(drain.opened_effects.len(), 1, "only new suffix intents");
        assert_eq!(
            drain.tail.intents.len(),
            1,
            "tail scan should match service slice"
        );

        let opened = &drain.opened_effects[0];
        assert_eq!(opened.record.kind, aos_effects::EffectKind::TIMER_SET);
        let params: TimerSetParams = serde_cbor::from_slice(&opened.intent.params_cbor)
            .expect("decode materialized timer params");
        assert_eq!(params.deliver_at_ns, 20);
        assert_eq!(params.key.as_deref(), Some("t-20"));
    }

    #[test]
    fn accept_input_enqueues_domain_event_without_scanning_open_effects() {
        let mut kernel = crate::world::test_support::minimal_kernel_with_router();
        let payload = serde_cbor::to_vec(&serde_cbor::Value::Map(BTreeMap::from([(
            serde_cbor::Value::Text("id".into()),
            serde_cbor::Value::Text("evt-1".into()),
        )])))
        .expect("encode event");

        kernel
            .accept(WorldInput::DomainEvent(DomainEvent::new(
                "com.acme/Event@1",
                payload,
            )))
            .expect("accept domain event");

        assert_eq!(
            kernel.workflow_queue.len(),
            1,
            "event should route into workflow queue"
        );
        assert!(
            !kernel.dump_journal().expect("journal").is_empty(),
            "accepted input should append journal state"
        );
    }

    #[test]
    fn apply_control_and_drain_returns_control_outcome_and_tail() {
        let store = Arc::new(MemStore::default());
        let loaded = LoadedManifest {
            manifest: minimal_manifest(),
            secrets: vec![],
            modules: HashMap::new(),
            effects: HashMap::new(),
            schemas: HashMap::new(),
            effect_catalog: EffectCatalog::new(),
        };
        let mut kernel =
            Kernel::from_loaded_manifest(store, loaded, Journal::new()).expect("empty kernel");

        let tail_start = kernel.journal_head();
        let control = kernel
            .apply_control(WorldControl::ApplyPatchDirect {
                patch: ManifestPatch {
                    manifest: minimal_manifest(),
                    nodes: Vec::new(),
                },
            })
            .expect("apply empty patch directly");
        let drain = kernel
            .drain_until_idle_from(tail_start)
            .expect("drain after control");

        assert!(matches!(control, WorldControlOutcome::PatchApplied { .. }));
        assert!(drain.kernel_idle, "control service should end idle");
    }

    #[test]
    fn governance_world_controls_run_through_typed_kernel_boundary() {
        let store = Arc::new(MemStore::default());
        let loaded = LoadedManifest {
            manifest: minimal_manifest(),
            secrets: vec![],
            modules: HashMap::new(),
            effects: HashMap::new(),
            schemas: HashMap::new(),
            effect_catalog: EffectCatalog::new(),
        };
        let mut kernel =
            Kernel::from_loaded_manifest(store, loaded, Journal::new()).expect("empty kernel");

        let patch = ManifestPatch {
            manifest: minimal_manifest(),
            nodes: Vec::new(),
        };
        let submitted = kernel
            .apply_control(WorldControl::SubmitProposal {
                patch,
                description: Some("typed proposal".into()),
            })
            .expect("submit proposal");
        let proposal_id = match submitted {
            WorldControlOutcome::ProposalSubmitted { proposal_id } => proposal_id,
            other => panic!("unexpected outcome: {other:?}"),
        };

        let shadowed = kernel
            .apply_control(WorldControl::RunShadow { proposal_id })
            .expect("run shadow");
        assert!(matches!(
            shadowed,
            WorldControlOutcome::ShadowRun {
                proposal_id: returned_id
            } if returned_id == proposal_id
        ));

        let decided = kernel
            .apply_control(WorldControl::DecideProposal {
                proposal_id,
                approver: "tester".into(),
                decision: ApprovalDecisionRecord::Approve,
            })
            .expect("approve proposal");
        assert!(matches!(
            decided,
            WorldControlOutcome::ProposalDecided {
                proposal_id: returned_id,
                decision: ApprovalDecisionRecord::Approve,
            } if returned_id == proposal_id
        ));

        let applied = kernel
            .apply_control(WorldControl::ApplyProposal { proposal_id })
            .expect("apply proposal");
        assert!(matches!(
            applied,
            WorldControlOutcome::ProposalApplied {
                proposal_id: returned_id
            } if returned_id == proposal_id
        ));
    }
}
