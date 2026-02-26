use super::*;
use serde::Serialize;

impl<S: Store + 'static> Kernel<S> {
    pub fn tick(&mut self) -> Result<(), KernelError> {
        if let Some(event) = self.reducer_queue.pop_front() {
            self.handle_reducer_event(event)?;
        }
        Ok(())
    }

    pub fn tick_until_idle(&mut self) -> Result<(), KernelError> {
        while !self.reducer_queue.is_empty() {
            self.tick()?;
        }
        Ok(())
    }

    pub fn drain_effects(&mut self) -> Result<Vec<aos_effects::EffectIntent>, KernelError> {
        self.effect_manager.drain()
    }

    pub fn has_pending_effects(&self) -> bool {
        self.effect_manager.has_pending()
    }

    pub fn restore_effect_queue(&mut self, intents: Vec<aos_effects::EffectIntent>) {
        self.effect_manager.restore_queue(intents);
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

        if let Some(context) = self
            .pending_reducer_receipts
            .get(&receipt.intent_hash)
            .cloned()
        {
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
        context: &ReducerEffectContext,
        receipt: &aos_effects::EffectReceipt,
        stamp: &IngressStamp,
    ) -> Result<(), KernelError> {
        let reducer_name = context.origin_module_id.clone();
        let (reducer_event_schema_name, reducer_event_schema) =
            self.resolve_workflow_event_schema(&reducer_name)?;

        let generic_event = crate::receipts::build_workflow_receipt_event(context, receipt)?;
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
                    crate::receipts::build_legacy_reducer_receipt_event(context, receipt)?
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
        self.reducer_queue.push_back(ReducerEvent {
            reducer: reducer_name.clone(),
            event,
            stamp: stamp.clone(),
        });
        Ok(())
    }

    fn resolve_workflow_event_schema<'a>(
        &'a self,
        reducer_name: &str,
    ) -> Result<(String, &'a TypeExpr), KernelError> {
        let module_def = self
            .module_defs
            .get(reducer_name)
            .ok_or_else(|| KernelError::ReducerNotFound(reducer_name.to_string()))?;
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
        Ok((reducer_event_schema_name, reducer_event_schema))
    }

    fn settle_workflow_receipt_intent(
        &mut self,
        context: &ReducerEffectContext,
        intent_hash: [u8; 32],
    ) {
        self.pending_reducer_receipts.remove(&intent_hash);
        self.mark_workflow_receipt_settled(
            &context.origin_module_id,
            context.origin_instance_key.as_deref(),
            intent_hash,
        );
        self.remember_receipt(intent_hash);
    }

    fn fail_workflow_instance(
        &mut self,
        context: &ReducerEffectContext,
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

    fn clear_pending_receipts_for_workflow_instance(
        &mut self,
        context: &ReducerEffectContext,
    ) -> usize {
        let pending_hashes: Vec<[u8; 32]> = self
            .pending_reducer_receipts
            .iter()
            .filter_map(|(hash, pending_ctx)| {
                if pending_ctx.origin_module_id == context.origin_module_id
                    && pending_ctx.origin_instance_key == context.origin_instance_key
                {
                    Some(*hash)
                } else {
                    None
                }
            })
            .collect();
        for hash in &pending_hashes {
            self.settle_workflow_receipt_intent(context, *hash);
        }
        pending_hashes.len()
    }

    fn try_deliver_workflow_rejected_receipt_event(
        &mut self,
        context: &ReducerEffectContext,
        receipt: &aos_effects::EffectReceipt,
        stamp: &IngressStamp,
        error_code: &str,
        error_message: &str,
    ) -> Result<bool, KernelError> {
        let reducer_name = context.origin_module_id.clone();
        let (reducer_event_schema_name, reducer_event_schema) =
            self.resolve_workflow_event_schema(&reducer_name)?;
        let rejected_event = crate::receipts::build_workflow_receipt_rejected_event(
            context,
            receipt,
            error_code,
            error_message,
        )?;
        let rejected_value: CborValue = serde_cbor::from_slice(&rejected_event.value)
            .map_err(|err| KernelError::ReceiptDecode(err.to_string()))?;
        let Ok(normalized) = try_normalize_receipt_payload(
            reducer_event_schema,
            &self.schema_index,
            rejected_value,
            crate::receipts::SYS_EFFECT_RECEIPT_REJECTED_SCHEMA,
        ) else {
            return Ok(false);
        };

        let mut event = DomainEvent::new(reducer_event_schema_name, normalized.bytes);
        event.key = context.origin_instance_key.clone();
        self.reducer_queue.push_back(ReducerEvent {
            reducer: reducer_name,
            event,
            stamp: stamp.clone(),
        });
        Ok(true)
    }

    fn handle_workflow_receipt_fault(
        &mut self,
        context: &ReducerEffectContext,
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
            let dropped = self.clear_pending_receipts_for_workflow_instance(context);
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
        context: &ReducerEffectContext,
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
        match aos_air_types::value_normalize::normalize_value_with_schema(
            candidate,
            reducer_event_schema,
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
