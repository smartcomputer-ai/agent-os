use aos_cbor::Hash;
use aos_effects::builtins::{
    BlobGetParams, BlobGetReceipt, BlobPutParams, BlobPutReceipt, TimerSetParams, TimerSetReceipt,
};
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_wasm_abi::DomainEvent;
use serde::Serialize;

use crate::error::KernelError;

pub const SYS_EFFECT_RECEIPT_ENVELOPE_SCHEMA: &str = "sys/EffectReceiptEnvelope@1";
const SYS_TIMER_FIRED_SCHEMA: &str = "sys/TimerFired@1";
const SYS_BLOB_PUT_RESULT_SCHEMA: &str = "sys/BlobPutResult@1";
const SYS_BLOB_GET_RESULT_SCHEMA: &str = "sys/BlobGetResult@1";

/// Metadata describing a workflow/reducer-origin effect that is awaiting a receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReducerEffectContext {
    pub origin_module_id: String,
    pub origin_instance_key: Option<Vec<u8>>,
    pub effect_kind: String,
    pub params_cbor: Vec<u8>,
    pub intent_id: [u8; 32],
    pub emitted_at_seq: u64,
    pub module_version: Option<String>,
}

impl ReducerEffectContext {
    pub fn new(
        origin_module_id: String,
        origin_instance_key: Option<Vec<u8>>,
        effect_kind: String,
        params_cbor: Vec<u8>,
        intent_id: [u8; 32],
        emitted_at_seq: u64,
        module_version: Option<String>,
    ) -> Self {
        Self {
            origin_module_id,
            origin_instance_key,
            effect_kind,
            params_cbor,
            intent_id,
            emitted_at_seq,
            module_version,
        }
    }
}

#[derive(Serialize)]
struct WorkflowReceiptEnvelope {
    origin_module_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    origin_instance_key: Option<Vec<u8>>,
    intent_id: String,
    effect_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    params_hash: Option<String>,
    #[serde(with = "serde_bytes")]
    receipt_payload: Vec<u8>,
    status: ReceiptStatus,
    emitted_at_seq: u64,
    adapter_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_cents: Option<u64>,
    #[serde(with = "serde_bytes")]
    signature: Vec<u8>,
}

#[derive(Serialize)]
struct TimerReceiptEvent {
    intent_hash: String,
    reducer: String,
    effect_kind: String,
    adapter_id: String,
    status: ReceiptStatus,
    requested: TimerSetParams,
    receipt: TimerSetReceipt,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_cents: Option<u64>,
    #[serde(with = "serde_bytes")]
    signature: Vec<u8>,
}

#[derive(Serialize)]
struct BlobReceiptEvent<TParams, TReceipt> {
    intent_hash: String,
    reducer: String,
    effect_kind: String,
    adapter_id: String,
    status: ReceiptStatus,
    requested: TParams,
    receipt: TReceipt,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_cents: Option<u64>,
    #[serde(with = "serde_bytes")]
    signature: Vec<u8>,
}

pub fn build_workflow_receipt_event(
    ctx: &ReducerEffectContext,
    receipt: &EffectReceipt,
) -> Result<DomainEvent, KernelError> {
    let payload = WorkflowReceiptEnvelope {
        origin_module_id: ctx.origin_module_id.clone(),
        origin_instance_key: ctx.origin_instance_key.clone(),
        intent_id: hash_to_hex(&ctx.intent_id),
        effect_kind: ctx.effect_kind.clone(),
        params_hash: Some(Hash::of_bytes(&ctx.params_cbor).to_hex()),
        receipt_payload: receipt.payload_cbor.clone(),
        status: receipt.status.clone(),
        emitted_at_seq: ctx.emitted_at_seq,
        adapter_id: receipt.adapter_id.clone(),
        cost_cents: receipt.cost_cents,
        signature: receipt.signature.clone(),
    };
    encode_event(SYS_EFFECT_RECEIPT_ENVELOPE_SCHEMA, payload)
}

/// Optional compatibility path for typed timer/blob receipt envelopes.
pub fn build_legacy_reducer_receipt_event(
    ctx: &ReducerEffectContext,
    receipt: &EffectReceipt,
) -> Result<Option<DomainEvent>, KernelError> {
    let event = match ctx.effect_kind.as_str() {
        aos_effects::EffectKind::TIMER_SET => Some(build_timer_event(ctx, receipt)?),
        aos_effects::EffectKind::BLOB_PUT => Some(build_blob_put_event(ctx, receipt)?),
        aos_effects::EffectKind::BLOB_GET => Some(build_blob_get_event(ctx, receipt)?),
        _ => None,
    };
    Ok(event)
}

fn build_timer_event(
    ctx: &ReducerEffectContext,
    receipt: &EffectReceipt,
) -> Result<DomainEvent, KernelError> {
    let requested: TimerSetParams = decode(&ctx.params_cbor)?;
    let timer_receipt: TimerSetReceipt = decode(&receipt.payload_cbor)?;
    let payload = TimerReceiptEvent {
        intent_hash: hash_to_hex(&ctx.intent_id),
        reducer: ctx.origin_module_id.clone(),
        effect_kind: ctx.effect_kind.clone(),
        adapter_id: receipt.adapter_id.clone(),
        status: receipt.status.clone(),
        requested,
        receipt: timer_receipt,
        cost_cents: receipt.cost_cents,
        signature: receipt.signature.clone(),
    };
    encode_event(SYS_TIMER_FIRED_SCHEMA, payload)
}

fn build_blob_put_event(
    ctx: &ReducerEffectContext,
    receipt: &EffectReceipt,
) -> Result<DomainEvent, KernelError> {
    let requested: BlobPutParams = decode(&ctx.params_cbor)?;
    let blob_receipt: BlobPutReceipt = decode(&receipt.payload_cbor)?;
    let payload = BlobReceiptEvent {
        intent_hash: hash_to_hex(&ctx.intent_id),
        reducer: ctx.origin_module_id.clone(),
        effect_kind: ctx.effect_kind.clone(),
        adapter_id: receipt.adapter_id.clone(),
        status: receipt.status.clone(),
        requested,
        receipt: blob_receipt,
        cost_cents: receipt.cost_cents,
        signature: receipt.signature.clone(),
    };
    encode_event(SYS_BLOB_PUT_RESULT_SCHEMA, payload)
}

fn build_blob_get_event(
    ctx: &ReducerEffectContext,
    receipt: &EffectReceipt,
) -> Result<DomainEvent, KernelError> {
    let requested: BlobGetParams = decode(&ctx.params_cbor)?;
    let blob_receipt: BlobGetReceipt = decode(&receipt.payload_cbor)?;
    let payload = BlobReceiptEvent {
        intent_hash: hash_to_hex(&ctx.intent_id),
        reducer: ctx.origin_module_id.clone(),
        effect_kind: ctx.effect_kind.clone(),
        adapter_id: receipt.adapter_id.clone(),
        status: receipt.status.clone(),
        requested,
        receipt: blob_receipt,
        cost_cents: receipt.cost_cents,
        signature: receipt.signature.clone(),
    };
    encode_event(SYS_BLOB_GET_RESULT_SCHEMA, payload)
}

fn encode_event<T: Serialize>(
    receipt_schema: &str,
    payload: T,
) -> Result<DomainEvent, KernelError> {
    let bytes =
        serde_cbor::to_vec(&payload).map_err(|err| KernelError::ReceiptDecode(err.to_string()))?;
    Ok(DomainEvent::new(receipt_schema, bytes))
}

fn hash_to_hex(bytes: &[u8; 32]) -> String {
    Hash::from_bytes(bytes)
        .map(|h| h.to_hex())
        .unwrap_or_else(|_| hex::encode(bytes))
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, KernelError> {
    serde_cbor::from_slice(bytes).map_err(|err| KernelError::ReceiptDecode(err.to_string()))
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
    use aos_air_types::HashRef;
    use aos_effects::EffectReceipt;
    use serde::Deserialize;

    fn fake_hash(byte: u8) -> HashRef {
        let hex = format!("{:02x}", byte);
        HashRef::new(format!("sha256:{}", hex.repeat(32))).unwrap()
    }

    fn base_receipt() -> EffectReceipt {
        EffectReceipt {
            intent_hash: [1u8; 32],
            adapter_id: "adapter.test".into(),
            status: ReceiptStatus::Ok,
            payload_cbor: vec![],
            cost_cents: Some(5),
            signature: vec![9, 9],
        }
    }

    fn base_context(effect_kind: &str) -> ReducerEffectContext {
        ReducerEffectContext::new(
            "com.acme/Workflow@1".into(),
            Some(b"order-123".to_vec()),
            effect_kind.into(),
            vec![],
            [7u8; 32],
            42,
            Some("sha256:deadbeef".into()),
        )
    }

    #[test]
    fn workflow_receipt_event_is_structured() {
        let mut ctx = base_context(aos_effects::EffectKind::HTTP_REQUEST);
        ctx.params_cbor = vec![1, 2, 3];
        let mut receipt = base_receipt();
        receipt.payload_cbor = vec![4, 5, 6];
        let event = build_workflow_receipt_event(&ctx, &receipt).expect("event");
        assert_eq!(event.schema, SYS_EFFECT_RECEIPT_ENVELOPE_SCHEMA);

        #[derive(Deserialize)]
        struct Payload {
            origin_module_id: String,
            #[serde(default, with = "super::serde_bytes_opt")]
            origin_instance_key: Option<Vec<u8>>,
            intent_id: String,
            effect_kind: String,
            params_hash: Option<String>,
            #[serde(with = "serde_bytes")]
            receipt_payload: Vec<u8>,
            status: ReceiptStatus,
            emitted_at_seq: u64,
            adapter_id: String,
            cost_cents: Option<u64>,
            #[serde(with = "serde_bytes")]
            signature: Vec<u8>,
        }

        let decoded: Payload = serde_cbor::from_slice(&event.value).unwrap();
        assert_eq!(decoded.origin_module_id, "com.acme/Workflow@1");
        assert_eq!(decoded.origin_instance_key, Some(b"order-123".to_vec()));
        assert_eq!(decoded.intent_id, hash_to_hex(&[7u8; 32]));
        assert_eq!(decoded.effect_kind, aos_effects::EffectKind::HTTP_REQUEST);
        assert_eq!(decoded.receipt_payload, vec![4, 5, 6]);
        assert_eq!(decoded.emitted_at_seq, 42);
        assert_eq!(decoded.adapter_id, "adapter.test");
        assert_eq!(decoded.status, ReceiptStatus::Ok);
        assert_eq!(decoded.cost_cents, Some(5));
        assert_eq!(decoded.signature, vec![9, 9]);
        assert!(decoded.params_hash.is_some());
    }

    #[test]
    fn legacy_unknown_effect_returns_none() {
        let ctx = base_context("custom.effect");
        let receipt = base_receipt();
        let legacy = build_legacy_reducer_receipt_event(&ctx, &receipt).expect("legacy");
        assert!(legacy.is_none());
    }

    #[test]
    fn timer_legacy_receipt_event_is_structured() {
        let params = TimerSetParams {
            deliver_at_ns: 99,
            key: Some("order-123".into()),
        };
        let mut ctx = base_context(aos_effects::EffectKind::TIMER_SET);
        ctx.params_cbor = serde_cbor::to_vec(&params).unwrap();
        let timer_receipt = TimerSetReceipt {
            delivered_at_ns: 123,
            key: Some("order-123".into()),
        };
        let mut receipt = base_receipt();
        receipt.payload_cbor = serde_cbor::to_vec(&timer_receipt).unwrap();

        let event = build_legacy_reducer_receipt_event(&ctx, &receipt)
            .expect("legacy")
            .expect("timer event");
        assert_eq!(event.schema, SYS_TIMER_FIRED_SCHEMA);
    }

    #[test]
    fn blob_put_legacy_receipt_event_is_structured() {
        let params = BlobPutParams {
            bytes: Vec::new(),
            blob_ref: Some(fake_hash(0x10)),
            refs: Some(vec![]),
        };
        let mut ctx = base_context(aos_effects::EffectKind::BLOB_PUT);
        ctx.params_cbor = serde_cbor::to_vec(&params).unwrap();
        let receipt_body = BlobPutReceipt {
            blob_ref: fake_hash(0x11),
            edge_ref: fake_hash(0x12),
            size: 42,
        };
        let mut receipt = base_receipt();
        receipt.payload_cbor = serde_cbor::to_vec(&receipt_body).unwrap();
        let event = build_legacy_reducer_receipt_event(&ctx, &receipt)
            .expect("legacy")
            .expect("blob.put event");
        assert_eq!(event.schema, SYS_BLOB_PUT_RESULT_SCHEMA);
    }

    #[test]
    fn blob_get_legacy_receipt_event_is_structured() {
        let params = BlobGetParams {
            blob_ref: fake_hash(0x10),
        };
        let mut ctx = base_context(aos_effects::EffectKind::BLOB_GET);
        ctx.params_cbor = serde_cbor::to_vec(&params).unwrap();
        let receipt_body = BlobGetReceipt {
            blob_ref: fake_hash(0x12),
            size: 99,
            bytes: vec![0; 99],
        };
        let mut receipt = base_receipt();
        receipt.payload_cbor = serde_cbor::to_vec(&receipt_body).unwrap();
        let event = build_legacy_reducer_receipt_event(&ctx, &receipt)
            .expect("legacy")
            .expect("blob.get event");
        assert_eq!(event.schema, SYS_BLOB_GET_RESULT_SCHEMA);
    }
}
