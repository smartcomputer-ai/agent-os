use std::sync::Arc;

use aos_cbor::Hash;
use aos_effects::builtins::{BlobGetParams, BlobGetReceipt};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use aos_store::Store;
use async_trait::async_trait;

use super::traits::AsyncEffectAdapter;

pub struct BlobGetAdapter<S: Store> {
    store: Arc<S>,
}

impl<S: Store> BlobGetAdapter<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for BlobGetAdapter<S> {
    fn kind(&self) -> &str {
        aos_effects::EffectKind::BLOB_GET
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: BlobGetParams = serde_cbor::from_slice(&intent.params_cbor)?;
        let hash = Hash::from_hex_str(params.blob_ref.as_str())?;
        let bytes = self.store.get_blob(hash)?;
        let receipt = BlobGetReceipt {
            blob_ref: params.blob_ref,
            size: bytes.len() as u64,
            bytes,
        };
        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "host.blob.get".into(),
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt)?,
            cost_cents: Some(0),
            signature: vec![0; 64],
        })
    }
}
