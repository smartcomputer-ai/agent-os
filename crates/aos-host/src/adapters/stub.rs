use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use async_trait::async_trait;

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
                    payload_cbor: vec![],
                    cost_cents: Some(0),
                    signature: vec![0; 64],
                })
            }
        }
    };
}

stub_adapter!(StubTimerAdapter, "timer.set");
stub_adapter!(StubHttpAdapter, "http.request");
stub_adapter!(StubLlmAdapter, "llm.generate");
stub_adapter!(StubBlobAdapter, "blob.put");
stub_adapter!(StubBlobGetAdapter, "blob.get");
