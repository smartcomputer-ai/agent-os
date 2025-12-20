use aos_air_types::{HashRef, TypeExpr};
use aos_cbor::Hash;
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

pub fn build_reducer_receipt_event(
    ctx: &ReducerEffectContext,
    receipt: &EffectReceipt,
    reducer_event_schema: &str,
    reducer_event_type: &TypeExpr,
) -> Result<DomainEvent, KernelError> {
    match ctx.effect_kind.as_str() {
        aos_effects::EffectKind::TIMER_SET => {
            build_timer_event(ctx, receipt, reducer_event_schema, reducer_event_type)
        }
        aos_effects::EffectKind::BLOB_PUT => {
            build_blob_put_event(ctx, receipt, reducer_event_schema, reducer_event_type)
        }
        aos_effects::EffectKind::BLOB_GET => {
            build_blob_get_event(ctx, receipt, reducer_event_schema, reducer_event_type)
        }
        other => Err(KernelError::UnsupportedReducerReceipt(other.to_string())),
    }
}

fn build_timer_event(
    ctx: &ReducerEffectContext,
    receipt: &EffectReceipt,
    reducer_event_schema: &str,
    reducer_event_type: &TypeExpr,
) -> Result<DomainEvent, KernelError> {
    let requested: TimerSetParams = decode(&ctx.params_cbor)?;
    let timer_receipt: TimerSetReceipt = decode(&receipt.payload_cbor)?;
    let payload = TimerReceiptEvent {
        intent_hash: hash_to_hex(&receipt.intent_hash),
        reducer: ctx.reducer.clone(),
        effect_kind: ctx.effect_kind.clone(),
        adapter_id: receipt.adapter_id.clone(),
        status: receipt.status.clone(),
        requested,
        receipt: timer_receipt,
        cost_cents: receipt.cost_cents,
        signature: receipt.signature.clone(),
    };
    encode_event(
        SYS_TIMER_FIRED_SCHEMA,
        payload,
        reducer_event_schema,
        reducer_event_type,
    )
}

fn build_blob_put_event(
    ctx: &ReducerEffectContext,
    receipt: &EffectReceipt,
    reducer_event_schema: &str,
    reducer_event_type: &TypeExpr,
) -> Result<DomainEvent, KernelError> {
    let requested: BlobPutParams = decode(&ctx.params_cbor)?;
    let blob_receipt: BlobPutReceipt = decode(&receipt.payload_cbor)?;
    let payload = BlobReceiptEvent {
        intent_hash: hash_to_hex(&receipt.intent_hash),
        reducer: ctx.reducer.clone(),
        effect_kind: ctx.effect_kind.clone(),
        adapter_id: receipt.adapter_id.clone(),
        status: receipt.status.clone(),
        requested,
        receipt: blob_receipt,
        cost_cents: receipt.cost_cents,
        signature: receipt.signature.clone(),
    };
    encode_event(
        SYS_BLOB_PUT_RESULT_SCHEMA,
        payload,
        reducer_event_schema,
        reducer_event_type,
    )
}

fn build_blob_get_event(
    ctx: &ReducerEffectContext,
    receipt: &EffectReceipt,
    reducer_event_schema: &str,
    reducer_event_type: &TypeExpr,
) -> Result<DomainEvent, KernelError> {
    let requested: BlobGetParams = decode(&ctx.params_cbor)?;
    let blob_receipt: BlobGetReceipt = decode(&receipt.payload_cbor)?;
    let payload = BlobReceiptEvent {
        intent_hash: hash_to_hex(&receipt.intent_hash),
        reducer: ctx.reducer.clone(),
        effect_kind: ctx.effect_kind.clone(),
        adapter_id: receipt.adapter_id.clone(),
        status: receipt.status.clone(),
        requested,
        receipt: blob_receipt,
        cost_cents: receipt.cost_cents,
        signature: receipt.signature.clone(),
    };
    encode_event(
        SYS_BLOB_GET_RESULT_SCHEMA,
        payload,
        reducer_event_schema,
        reducer_event_type,
    )
}

fn encode_event<T: Serialize>(
    receipt_schema: &str,
    payload: T,
    reducer_event_schema: &str,
    reducer_event_type: &TypeExpr,
) -> Result<DomainEvent, KernelError> {
    let value = serde_cbor::to_value(&payload)
        .map_err(|err| KernelError::ReceiptDecode(err.to_string()))?;
    let bytes = wrap_payload_for_reducer_event(
        reducer_event_schema,
        reducer_event_type,
        receipt_schema,
        value,
    )?;
    Ok(DomainEvent::new(reducer_event_schema, bytes))
}

fn wrap_payload_for_reducer_event(
    reducer_event_schema: &str,
    reducer_event_type: &TypeExpr,
    receipt_schema: &str,
    payload: serde_cbor::Value,
) -> Result<Vec<u8>, KernelError> {
    if reducer_event_schema == receipt_schema {
        return serde_cbor::to_vec(&payload)
            .map_err(|err| KernelError::ReceiptDecode(err.to_string()));
    }
    let TypeExpr::Variant(variant) = reducer_event_type else {
        return Err(KernelError::ReducerReceiptSchemaMismatch {
            reducer_event_schema: reducer_event_schema.to_string(),
            receipt_schema: receipt_schema.to_string(),
            reason: "reducer event schema is not a variant".into(),
        });
    };
    let mut tag = None;
    for (name, ty) in &variant.variant {
        if let TypeExpr::Ref(reference) = ty {
            if reference.reference.as_str() == receipt_schema {
                if tag.is_some() {
                    return Err(KernelError::ReducerReceiptSchemaMismatch {
                        reducer_event_schema: reducer_event_schema.to_string(),
                        receipt_schema: receipt_schema.to_string(),
                        reason: "receipt schema appears in multiple variant arms".into(),
                    });
                }
                tag = Some(name.clone());
            }
        }
    }
    let tag = tag.ok_or_else(|| KernelError::ReducerReceiptSchemaMismatch {
        reducer_event_schema: reducer_event_schema.to_string(),
        receipt_schema: receipt_schema.to_string(),
        reason: "receipt schema not found in variant".into(),
    })?;
    let wrapped = serde_cbor::Value::Map(vec![
        (
            serde_cbor::Value::Text("$tag".into()),
            serde_cbor::Value::Text(tag),
        ),
        (
            serde_cbor::Value::Text("$value".into()),
            payload,
        ),
    ]);
    serde_cbor::to_vec(&wrapped).map_err(|err| KernelError::ReceiptDecode(err.to_string()))
}

fn hash_to_hex(bytes: &[u8; 32]) -> String {
    Hash::from_bytes(bytes)
        .map(|h| h.to_hex())
        .unwrap_or_else(|_| hex::encode(bytes))
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, KernelError> {
    serde_cbor::from_slice(bytes).map_err(|err| KernelError::ReceiptDecode(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_effects::EffectReceipt;
    use aos_air_types::{SchemaRef, TypeRef, TypeVariant};
    use indexmap::IndexMap;
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

    fn reducer_event_variant(schema: &str, tag: &str) -> (String, TypeExpr) {
        let mut variants = IndexMap::new();
        variants.insert(
            tag.to_string(),
            TypeExpr::Ref(TypeRef {
                reference: SchemaRef::new(schema).unwrap(),
            }),
        );
        (
            "com.acme/ReducerEvent@1".to_string(),
            TypeExpr::Variant(TypeVariant { variant: variants }),
        )
    }

    /// Rejects reducer receipts whose effect kind is not part of the built-in micro-effect set.
    #[test]
    fn rejects_unknown_effect_kind() {
        let ctx = ReducerEffectContext::new("reducer".into(), "unknown".into(), vec![]);
        let receipt = base_receipt();
        let (schema_name, schema_ty) = reducer_event_variant(SYS_TIMER_FIRED_SCHEMA, "Fired");
        let err =
            build_reducer_receipt_event(&ctx, &receipt, &schema_name, &schema_ty).unwrap_err();
        assert!(matches!(err, KernelError::UnsupportedReducerReceipt(_)));
    }

    /// Verifies timer receipts are wrapped into the reducer's variant schema.
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

        let (schema_name, schema_ty) = reducer_event_variant(SYS_TIMER_FIRED_SCHEMA, "Fired");
        let event =
            build_reducer_receipt_event(&ctx, &receipt, &schema_name, &schema_ty).expect("event");
        assert_eq!(event.schema, schema_name);

        #[derive(Deserialize)]
        struct EventPayload {
            intent_hash: String,
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

        let value: serde_cbor::Value = serde_cbor::from_slice(&event.value).unwrap();
        let serde_cbor::Value::Map(map) = value else {
            panic!("expected variant map");
        };
        let mut tag = None;
        let mut payload = None;
        for (key, val) in map {
            if key == serde_cbor::Value::Text("$tag".into()) {
                tag = Some(val);
            } else if key == serde_cbor::Value::Text("$value".into()) {
                payload = Some(val);
            }
        }
        assert_eq!(tag, Some(serde_cbor::Value::Text("Fired".into())));
        let payload = payload.expect("payload");
        let decoded: EventPayload = serde_cbor::value::from_value(payload).unwrap();
        assert_eq!(
            decoded.intent_hash,
            Hash::from_bytes(&receipt.intent_hash).unwrap().to_hex()
        );
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

    #[test]
    fn blob_put_receipt_event_is_structured() {
        let params = BlobPutParams {
            namespace: "ns".into(),
            blob_ref: fake_hash(0x10),
        };
        let ctx = ReducerEffectContext::new(
            "com.acme/Reducer@1".into(),
            aos_effects::EffectKind::BLOB_PUT.into(),
            serde_cbor::to_vec(&params).unwrap(),
        );
        let receipt_body = BlobPutReceipt {
            blob_ref: fake_hash(0x11),
            size: 42,
        };
        let mut receipt = base_receipt();
        receipt.payload_cbor = serde_cbor::to_vec(&receipt_body).unwrap();

        let (schema_name, schema_ty) = reducer_event_variant(SYS_BLOB_PUT_RESULT_SCHEMA, "Put");
        let event =
            build_reducer_receipt_event(&ctx, &receipt, &schema_name, &schema_ty).expect("event");
        assert_eq!(event.schema, schema_name);

        #[derive(Deserialize)]
        struct Payload {
            requested: BlobPutParams,
            receipt: BlobPutReceipt,
        }

        let value: serde_cbor::Value = serde_cbor::from_slice(&event.value).unwrap();
        let serde_cbor::Value::Map(map) = value else {
            panic!("expected variant map");
        };
        let payload = map
            .into_iter()
            .find_map(|(key, val)| {
                (key == serde_cbor::Value::Text("$value".into())).then_some(val)
            })
            .expect("payload");
        let decoded: Payload = serde_cbor::value::from_value(payload).unwrap();
        assert_eq!(decoded.requested.namespace, "ns");
        assert_eq!(decoded.receipt.size, 42);
    }

    #[test]
    fn blob_get_receipt_event_is_structured() {
        let params = BlobGetParams {
            namespace: "ns".into(),
            key: "doc".into(),
        };
        let ctx = ReducerEffectContext::new(
            "com.acme/Reducer@1".into(),
            aos_effects::EffectKind::BLOB_GET.into(),
            serde_cbor::to_vec(&params).unwrap(),
        );
        let receipt_body = BlobGetReceipt {
            blob_ref: fake_hash(0x12),
            size: 99,
        };
        let mut receipt = base_receipt();
        receipt.payload_cbor = serde_cbor::to_vec(&receipt_body).unwrap();

        let (schema_name, schema_ty) = reducer_event_variant(SYS_BLOB_GET_RESULT_SCHEMA, "Get");
        let event =
            build_reducer_receipt_event(&ctx, &receipt, &schema_name, &schema_ty).expect("event");
        assert_eq!(event.schema, schema_name);

        #[derive(Deserialize)]
        struct Payload {
            requested: BlobGetParams,
            receipt: BlobGetReceipt,
        }

        let value: serde_cbor::Value = serde_cbor::from_slice(&event.value).unwrap();
        let serde_cbor::Value::Map(map) = value else {
            panic!("expected variant map");
        };
        let payload = map
            .into_iter()
            .find_map(|(key, val)| {
                (key == serde_cbor::Value::Text("$value".into())).then_some(val)
            })
            .expect("payload");
        let decoded: Payload = serde_cbor::value::from_value(payload).unwrap();
        assert_eq!(decoded.requested.key, "doc");
        assert_eq!(decoded.receipt.blob_ref, receipt_body.blob_ref);
    }
}
