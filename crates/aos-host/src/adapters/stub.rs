use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::traits::AsyncEffectAdapter;

macro_rules! stub_adapter {
    ($name:ident, $kind:expr) => {
        pub struct $name;

        #[async_trait]
        impl AsyncEffectAdapter for $name {
            fn kind(&self) -> &str {
                $kind
            }

            async fn execute(
                &self,
                intent: &EffectIntent,
            ) -> anyhow::Result<aos_effects::EffectReceipt> {
                Ok(EffectReceipt {
                    intent_hash: intent.intent_hash,
                    adapter_id: $kind.to_string(),
                    status: ReceiptStatus::Ok,
                    // CBOR empty map 0xa0 = {} - valid payload for most adapters
                    payload_cbor: vec![0xa0],
                    cost_cents: Some(0),
                    signature: vec![0; 64],
                })
            }
        }
    };
}

stub_adapter!(StubHttpAdapter, "http.request");
stub_adapter!(StubLlmAdapter, "llm.generate");
stub_adapter!(StubBlobAdapter, "blob.put");
stub_adapter!(StubBlobGetAdapter, "blob.get");

// Timer adapter requires a specific receipt format
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimerSetParams {
    deliver_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimerSetReceipt {
    delivered_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

pub struct StubTimerAdapter;

#[async_trait]
impl AsyncEffectAdapter for StubTimerAdapter {
    fn kind(&self) -> &str {
        "timer.set"
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        // Parse the requested timer params to get deliver_at_ns
        let params: TimerSetParams =
            serde_cbor::from_slice(&intent.params_cbor).unwrap_or(TimerSetParams {
                deliver_at_ns: 0,
                key: None,
            });

        // Create a receipt that says the timer fired at the requested time
        let receipt_payload = TimerSetReceipt {
            delivered_at_ns: params.deliver_at_ns,
            key: params.key,
        };

        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "timer.set".to_string(),
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt_payload)?,
            cost_cents: Some(0),
            signature: vec![0; 64],
        })
    }
}
