use anyhow::Context;
use aos_air_types::HashRef;
use aos_effects::builtins::{
    BlobGetParams, BlobGetReceipt, BlobPutParams, BlobPutReceipt, HeaderMap, HttpRequestParams,
    HttpRequestReceipt, LlmFinishReason, LlmGenerateParams, LlmGenerateReceipt, RequestTimings,
    TimerSetParams, TimerSetReceipt, TokenUsage,
};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use async_trait::async_trait;
use serde::Serialize;

use super::traits::AsyncEffectAdapter;

pub struct StubHttpAdapter;

#[async_trait]
impl AsyncEffectAdapter for StubHttpAdapter {
    fn kind(&self) -> &str {
        "http.request"
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let _: HttpRequestParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode http.request params")?;
        let receipt_payload = HttpRequestReceipt {
            status: 200,
            headers: HeaderMap::new(),
            body_ref: None,
            timings: RequestTimings {
                start_ns: 0,
                end_ns: 0,
            },
            adapter_id: "stub.http".into(),
        };
        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "stub.http".to_string(),
            status: ReceiptStatus::Ok,
            payload_cbor: encode_receipt_payload("http.request", &receipt_payload)?,
            cost_cents: Some(0),
            signature: vec![0; 64],
        })
    }
}

pub struct StubLlmAdapter;

#[async_trait]
impl AsyncEffectAdapter for StubLlmAdapter {
    fn kind(&self) -> &str {
        "llm.generate"
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: LlmGenerateParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode llm.generate params")?;
        let receipt_payload = LlmGenerateReceipt {
            output_ref: fake_hashref(0x31),
            raw_output_ref: None,
            provider_response_id: None,
            finish_reason: LlmFinishReason {
                reason: "stub".into(),
                raw: None,
            },
            token_usage: TokenUsage {
                prompt: 0,
                completion: 0,
                total: Some(0),
            },
            usage_details: None,
            warnings_ref: None,
            rate_limit_ref: None,
            cost_cents: Some(0),
            provider_id: params.provider,
        };
        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "stub.llm".to_string(),
            status: ReceiptStatus::Ok,
            payload_cbor: encode_receipt_payload("llm.generate", &receipt_payload)?,
            cost_cents: Some(0),
            signature: vec![0; 64],
        })
    }
}

pub struct StubBlobAdapter;

#[async_trait]
impl AsyncEffectAdapter for StubBlobAdapter {
    fn kind(&self) -> &str {
        "blob.put"
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: BlobPutParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode blob.put params")?;
        let receipt_payload = BlobPutReceipt {
            blob_ref: params.blob_ref.unwrap_or_else(|| fake_hashref(0x11)),
            edge_ref: fake_hashref(0x12),
            size: params.bytes.len() as u64,
        };
        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "stub.blob.put".to_string(),
            status: ReceiptStatus::Ok,
            payload_cbor: encode_receipt_payload("blob.put", &receipt_payload)?,
            cost_cents: Some(0),
            signature: vec![0; 64],
        })
    }
}

pub struct StubBlobGetAdapter;

#[async_trait]
impl AsyncEffectAdapter for StubBlobGetAdapter {
    fn kind(&self) -> &str {
        "blob.get"
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: BlobGetParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode blob.get params")?;
        let receipt_payload = BlobGetReceipt {
            blob_ref: params.blob_ref,
            size: 0,
            bytes: Vec::new(),
        };
        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "stub.blob.get".to_string(),
            status: ReceiptStatus::Ok,
            payload_cbor: encode_receipt_payload("blob.get", &receipt_payload)?,
            cost_cents: Some(0),
            signature: vec![0; 64],
        })
    }
}

pub struct StubTimerAdapter;

#[async_trait]
impl AsyncEffectAdapter for StubTimerAdapter {
    fn kind(&self) -> &str {
        "timer.set"
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: TimerSetParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode timer.set params")?;
        let receipt_payload = TimerSetReceipt {
            delivered_at_ns: params.deliver_at_ns,
            key: params.key,
        };

        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "stub.timer".to_string(),
            status: ReceiptStatus::Ok,
            payload_cbor: encode_receipt_payload("timer.set", &receipt_payload)?,
            cost_cents: Some(0),
            signature: vec![0; 64],
        })
    }
}

pub struct StubVaultPutAdapter;

#[async_trait]
impl AsyncEffectAdapter for StubVaultPutAdapter {
    fn kind(&self) -> &str {
        "vault.put"
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "stub.vault.put".to_string(),
            status: ReceiptStatus::Error,
            payload_cbor: vec![],
            cost_cents: Some(0),
            signature: vec![0; 64],
        })
    }
}

pub struct StubVaultRotateAdapter;

#[async_trait]
impl AsyncEffectAdapter for StubVaultRotateAdapter {
    fn kind(&self) -> &str {
        "vault.rotate"
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "stub.vault.rotate".to_string(),
            status: ReceiptStatus::Error,
            payload_cbor: vec![],
            cost_cents: Some(0),
            signature: vec![0; 64],
        })
    }
}

fn fake_hashref(byte: u8) -> HashRef {
    let hex = format!("{:02x}", byte);
    HashRef::new(format!("sha256:{}", hex.repeat(32))).expect("valid stub hash ref")
}

fn encode_receipt_payload<T: Serialize>(kind: &str, payload: &T) -> anyhow::Result<Vec<u8>> {
    serde_cbor::to_vec(payload).with_context(|| format!("encode {kind} receipt payload"))
}
