use anyhow::Context;
use aos_air_types::HashRef;
use aos_effects::builtins::{
    BlobGetParams, BlobGetReceipt, BlobPutParams, BlobPutReceipt, HeaderMap, HttpRequestParams,
    HttpRequestReceipt, LlmCompactParams, LlmCompactReceipt, LlmCompactionArtifactKind,
    LlmCountTokensParams, LlmCountTokensReceipt, LlmFinishReason, LlmGenerateParams,
    LlmGenerateReceipt, LlmTokenCountByRef, LlmTokenCountQuality, LlmWindowItem, LlmWindowItemKind,
    RequestTimings, TimerSetParams, TimerSetReceipt, TokenUsage,
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

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
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
        };
        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
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

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: LlmGenerateParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode llm.generate params")?;
        let receipt_payload = LlmGenerateReceipt {
            output_ref: fake_hashref(0x31),
            raw_output_ref: None,
            provider_response_id: None,
            provider_context_items: Vec::new(),
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
            status: ReceiptStatus::Ok,
            payload_cbor: encode_receipt_payload("llm.generate", &receipt_payload)?,
            cost_cents: Some(0),
            signature: vec![0; 64],
        })
    }
}

pub struct StubLlmCompactAdapter;

#[async_trait]
impl AsyncEffectAdapter for StubLlmCompactAdapter {
    fn kind(&self) -> &str {
        "llm.compact"
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: LlmCompactParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode llm.compact params")?;
        let artifact_ref = fake_hashref(0x41);
        let active_window_items =
            if params.preserve_window_items.is_empty() && params.recent_tail_items.is_empty() {
                vec![LlmWindowItem {
                    item_id: format!("compact:{}:summary", params.operation_id),
                    kind: LlmWindowItemKind::AosSummaryRef,
                    ref_: artifact_ref.clone(),
                    lane: Some("Summary".into()),
                    source_range: params.source_range.clone(),
                    source_refs: params
                        .source_window_items
                        .iter()
                        .map(|item| item.ref_.clone())
                        .collect(),
                    provider_compatibility: None,
                    estimated_tokens: params.target_tokens,
                    metadata: Default::default(),
                }]
            } else {
                params
                    .preserve_window_items
                    .iter()
                    .chain(params.recent_tail_items.iter())
                    .cloned()
                    .collect()
            };
        let receipt_payload = LlmCompactReceipt {
            operation_id: params.operation_id,
            artifact_kind: LlmCompactionArtifactKind::AosSummary,
            artifact_refs: vec![artifact_ref],
            source_range: params.source_range,
            compacted_through: None,
            active_window_items,
            token_usage: Some(TokenUsage {
                prompt: 0,
                completion: 0,
                total: Some(0),
            }),
            provider_metadata_ref: None,
            warnings_ref: None,
            provider_id: params.provider,
        };
        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            status: ReceiptStatus::Ok,
            payload_cbor: encode_receipt_payload("llm.compact", &receipt_payload)?,
            cost_cents: Some(0),
            signature: vec![0; 64],
        })
    }
}

pub struct StubLlmCountTokensAdapter;

#[async_trait]
impl AsyncEffectAdapter for StubLlmCountTokensAdapter {
    fn kind(&self) -> &str {
        "llm.count_tokens"
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: LlmCountTokensParams = serde_cbor::from_slice(&intent.params_cbor)
            .context("decode llm.count_tokens params")?;
        let input_tokens = params
            .window_items
            .iter()
            .map(|item| item.estimated_tokens.unwrap_or(0))
            .sum::<u64>();
        let counts_by_ref = params
            .window_items
            .iter()
            .map(|item| LlmTokenCountByRef {
                ref_: item.ref_.clone(),
                tokens: item.estimated_tokens.unwrap_or(0),
                quality: LlmTokenCountQuality::LocalEstimate,
            })
            .collect();
        let receipt_payload = LlmCountTokensReceipt {
            input_tokens: Some(input_tokens),
            original_input_tokens: Some(input_tokens),
            counts_by_ref,
            tool_tokens: None,
            response_format_tokens: None,
            quality: LlmTokenCountQuality::LocalEstimate,
            provider: params.provider,
            model: params.model,
            candidate_plan_id: params.candidate_plan_id,
            provider_metadata_ref: None,
            warnings_ref: None,
        };
        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            status: ReceiptStatus::Ok,
            payload_cbor: encode_receipt_payload("llm.count_tokens", &receipt_payload)?,
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

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: BlobPutParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode blob.put params")?;
        let receipt_payload = BlobPutReceipt {
            blob_ref: params.blob_ref.unwrap_or_else(|| fake_hashref(0x11)),
            edge_ref: fake_hashref(0x12),
            size: params.bytes.len() as u64,
        };
        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
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

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: BlobGetParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode blob.get params")?;
        let receipt_payload = BlobGetReceipt {
            blob_ref: params.blob_ref,
            size: 0,
            bytes: Vec::new(),
        };
        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
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

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: TimerSetParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode timer.set params")?;
        let receipt_payload = TimerSetReceipt {
            delivered_at_ns: params.deliver_at_ns,
            key: params.key,
        };

        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
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

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
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

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
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
