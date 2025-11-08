use aos_effects::builtins::{
    BlobGetParams, BlobGetReceipt, BlobPutParams, BlobPutReceipt, TimerSetParams, TimerSetReceipt,
};
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_wasm_abi::DomainEvent;
use serde::Serialize;

use crate::error::KernelError;

const SYS_TIMER_FIRED_SCHEMA: &str = "sys/TimerFired@1";
const SYS_BLOB_PUT_RESULT_SCHEMA: &str = "sys/BlobPutResult@1";
const SYS_BLOB_GET_RESULT_SCHEMA: &str = "sys/BlobGetResult@1";

/// Metadata describing a reducer-origin effect that is awaiting a receipt.
#[derive(Clone)]
pub struct ReducerEffectContext {
    pub reducer: String,
    pub effect_kind: String,
    pub params_cbor: Vec<u8>,
}

impl ReducerEffectContext {
    pub fn new(reducer: String, effect_kind: String, params_cbor: Vec<u8>) -> Self {
        Self {
            reducer,
            effect_kind,
            params_cbor,
        }
    }
}

#[derive(Serialize)]
struct TimerReceiptEvent {
    #[serde(with = "serde_bytes")]
    intent_hash: [u8; 32],
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
    #[serde(with = "serde_bytes")]
    intent_hash: [u8; 32],
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

pub fn build_reducer_receipt_event(
    ctx: &ReducerEffectContext,
    receipt: &EffectReceipt,
) -> Result<DomainEvent, KernelError> {
    match ctx.effect_kind.as_str() {
        aos_effects::EffectKind::TIMER_SET => build_timer_event(ctx, receipt),
        aos_effects::EffectKind::BLOB_PUT => build_blob_put_event(ctx, receipt),
        aos_effects::EffectKind::BLOB_GET => build_blob_get_event(ctx, receipt),
        other => Err(KernelError::UnsupportedReducerReceipt(other.to_string())),
    }
}

fn build_timer_event(
    ctx: &ReducerEffectContext,
    receipt: &EffectReceipt,
) -> Result<DomainEvent, KernelError> {
    let requested: TimerSetParams = decode(&ctx.params_cbor)?;
    let timer_receipt: TimerSetReceipt = decode(&receipt.payload_cbor)?;
    let payload = TimerReceiptEvent {
        intent_hash: receipt.intent_hash,
        reducer: ctx.reducer.clone(),
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
        intent_hash: receipt.intent_hash,
        reducer: ctx.reducer.clone(),
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
        intent_hash: receipt.intent_hash,
        reducer: ctx.reducer.clone(),
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

fn encode_event<T: Serialize>(schema: &str, payload: T) -> Result<DomainEvent, KernelError> {
    let value =
        serde_cbor::to_vec(&payload).map_err(|err| KernelError::ReceiptDecode(err.to_string()))?;
    Ok(DomainEvent::new(schema, value))
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, KernelError> {
    serde_cbor::from_slice(bytes).map_err(|err| KernelError::ReceiptDecode(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_effects::EffectReceipt;
    use serde::Deserialize;

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

    #[test]
    fn rejects_unknown_effect_kind() {
        let ctx = ReducerEffectContext::new("reducer".into(), "unknown".into(), vec![]);
        let receipt = base_receipt();
        let err = build_reducer_receipt_event(&ctx, &receipt).unwrap_err();
        assert!(matches!(err, KernelError::UnsupportedReducerReceipt(_)));
    }

    #[test]
    fn timer_receipt_event_is_structured() {
        let params = TimerSetParams {
            deliver_at_ns: 99,
            key: Some("order-123".into()),
        };
        let ctx = ReducerEffectContext::new(
            "com.acme/Reducer@1".into(),
            aos_effects::EffectKind::TIMER_SET.into(),
            serde_cbor::to_vec(&params).unwrap(),
        );
        let timer_receipt = TimerSetReceipt {
            delivered_at_ns: 123,
            key: Some("order-123".into()),
        };
        let mut receipt = base_receipt();
        receipt.payload_cbor = serde_cbor::to_vec(&timer_receipt).unwrap();

        let event = build_reducer_receipt_event(&ctx, &receipt).expect("event");
        assert_eq!(event.schema, SYS_TIMER_FIRED_SCHEMA);

        #[derive(Deserialize)]
        struct EventPayload {
            #[serde(with = "serde_bytes")]
            intent_hash: Vec<u8>,
            reducer: String,
            effect_kind: String,
            adapter_id: String,
            status: ReceiptStatus,
            requested: TimerSetParams,
            receipt: TimerSetReceipt,
            cost_cents: Option<u64>,
            #[serde(with = "serde_bytes")]
            signature: Vec<u8>,
        }

        let decoded: EventPayload = serde_cbor::from_slice(&event.value).unwrap();
        assert_eq!(decoded.intent_hash, receipt.intent_hash);
        assert_eq!(decoded.reducer, "com.acme/Reducer@1");
        assert_eq!(decoded.effect_kind, aos_effects::EffectKind::TIMER_SET);
        assert_eq!(decoded.requested.deliver_at_ns, params.deliver_at_ns);
        assert_eq!(
            decoded.receipt.delivered_at_ns,
            timer_receipt.delivered_at_ns
        );
        assert_eq!(decoded.cost_cents, Some(5));
        assert_eq!(decoded.signature, vec![9, 9]);
    }
}
