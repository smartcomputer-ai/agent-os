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

    pub fn handle_stream_frame(
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

    pub(super) fn handle_stream_frame_with_ingress(
        &mut self,
        frame: aos_effects::EffectStreamFrame,
        stamp: IngressStamp,
    ) -> Result<(), KernelError> {
        Self::validate_entropy(&stamp.entropy)?;

        let Some(context) = self.pending_reducer_receipts.get(&frame.intent_hash).cloned() else {
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

    fn deliver_stream_frame_to_workflow_instance(
        &mut self,
        context: &ReducerEffectContext,
        frame: &aos_effects::EffectStreamFrame,
        stamp: &IngressStamp,
    ) -> Result<(), KernelError> {
        let reducer_name = context.origin_module_id.clone();
        let (reducer_event_schema_name, reducer_event_schema) =
            self.resolve_workflow_event_schema(&reducer_name)?;

        let stream_event = crate::receipts::build_workflow_stream_frame_event(context, frame)?;
        let stream_value: CborValue = serde_cbor::from_slice(&stream_event.value)
            .map_err(|err| KernelError::ReceiptDecode(err.to_string()))?;
        let normalized = try_normalize_receipt_payload(
            reducer_event_schema,
            &self.schema_index,
            stream_value,
            crate::receipts::SYS_EFFECT_STREAM_FRAME_SCHEMA,
        )
        .map_err(|err| {
            KernelError::Manifest(format!(
                "stream frame payload for '{reducer_name}' does not match event schema '{}': {err}",
                reducer_event_schema_name
            ))
        })?;

        let mut event = DomainEvent::new(reducer_event_schema_name, normalized.bytes);
        event.key = context.origin_instance_key.clone();
        self.reducer_queue.push_back(ReducerEvent {
            reducer: reducer_name,
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
    use crate::journal::JournalKind;
    use crate::journal::mem::MemJournal;
    use aos_air_types::{HashRef, ModuleAbi, ModuleKind, ReducerAbi, SchemaRef};
    use aos_effects::EffectStreamFrame;
    use aos_store::MemStore;
    use std::sync::Arc;

    fn install_stream_module(kernel: &mut Kernel<MemStore>, module_name: &str) {
        let module = aos_air_types::DefModule {
            name: module_name.into(),
            module_kind: ModuleKind::Workflow,
            wasm_hash: HashRef::new(format!("sha256:{}", "1".repeat(64))).unwrap(),
            key_schema: None,
            abi: ModuleAbi {
                reducer: Some(ReducerAbi {
                    state: SchemaRef::new("sys/PlanError@1").unwrap(),
                    event: SchemaRef::new(crate::receipts::SYS_EFFECT_STREAM_FRAME_SCHEMA).unwrap(),
                    context: None,
                    annotations: None,
                    effects_emitted: vec![],
                    cap_slots: Default::default(),
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
        let journal = Box::new(MemJournal::new());
        let mut kernel = crate::world::test_support::kernel_with_store_and_journal(store, journal);
        install_stream_module(&mut kernel, module_name);
        let context = ReducerEffectContext::new(
            module_name.into(),
            None,
            "http.request".into(),
            vec![1, 2, 3],
            intent_hash,
            emitted_at_seq,
            None,
        );
        kernel
            .pending_reducer_receipts
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
        kernel.handle_stream_frame(frame.clone()).expect("accept stream");
        assert_eq!(kernel.reducer_queue.len(), 1);
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

        kernel.reducer_queue.clear();
        kernel
            .handle_stream_frame(frame)
            .expect("duplicate frame should be dropped");
        assert!(
            kernel.reducer_queue.is_empty(),
            "duplicate frame must not enqueue reducer event"
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
        kernel.handle_stream_frame(frame).expect("accept stream gap");
        let entries = kernel.dump_journal().expect("journal");
        let mut has_gap = false;
        for entry in entries {
            if entry.kind != JournalKind::Custom {
                continue;
            }
            let record: crate::journal::JournalRecord = serde_cbor::from_slice(&entry.payload)
                .expect("decode custom record");
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
}
