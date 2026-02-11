use std::sync::Arc;

use aos_cbor::Hash;
use aos_effects::builtins::{BlobPutParams, BlobPutReceipt};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use aos_store::Store;
use async_trait::async_trait;

use super::traits::AsyncEffectAdapter;

pub struct BlobPutAdapter<S: Store> {
    store: Arc<S>,
}

impl<S: Store> BlobPutAdapter<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for BlobPutAdapter<S> {
    fn kind(&self) -> &str {
        aos_effects::EffectKind::BLOB_PUT
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: BlobPutParams = serde_cbor::from_slice(&intent.params_cbor)?;
        let expected = Hash::from_hex_str(params.blob_ref.as_str())?;
        let computed = Hash::of_bytes(&params.bytes);
        if expected != computed {
            return Ok(EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: "host.blob.put".into(),
                status: ReceiptStatus::Error,
                payload_cbor: serde_cbor::to_vec(&BlobPutReceipt {
                    blob_ref: params.blob_ref,
                    size: params.bytes.len() as u64,
                })?,
                cost_cents: Some(0),
                signature: vec![0; 64],
            });
        }

        let stored = self.store.put_blob(&params.bytes)?;
        let receipt = BlobPutReceipt {
            blob_ref: aos_air_types::HashRef::new(stored.to_hex())?,
            size: params.bytes.len() as u64,
        };

        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "host.blob.put".into(),
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt)?,
            cost_cents: Some(0),
            signature: vec![0; 64],
        })
    }
}
