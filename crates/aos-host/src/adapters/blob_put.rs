use std::sync::Arc;

use aos_cbor::{Hash, to_canonical_cbor};
use aos_effects::builtins::{BlobEdge, BlobPutParams, BlobPutReceipt};
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
        let refs = params.refs.unwrap_or_default();
        let expected_ref = params
            .blob_ref
            .ok_or_else(|| anyhow::anyhow!("blob.put params missing blob_ref"))?;
        let expected = Hash::from_hex_str(expected_ref.as_str())?;
        let computed = Hash::of_bytes(&params.bytes);
        let computed_ref = aos_air_types::HashRef::new(computed.to_hex())?;
        let edge = BlobEdge {
            blob_ref: computed_ref.clone(),
            refs,
        };
        let edge_bytes = to_canonical_cbor(&edge)?;
        let edge_hash = Hash::of_bytes(&edge_bytes);
        let edge_ref = aos_air_types::HashRef::new(edge_hash.to_hex())?;
        if expected != computed {
            return Ok(EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: "host.blob.put".into(),
                status: ReceiptStatus::Error,
                payload_cbor: serde_cbor::to_vec(&BlobPutReceipt {
                    blob_ref: computed_ref,
                    edge_ref,
                    size: params.bytes.len() as u64,
                })?,
                cost_cents: Some(0),
                signature: vec![0; 64],
            });
        }

        let stored = self.store.put_blob(&params.bytes)?;
        let edge_stored = self.store.put_blob(&edge_bytes)?;
        let receipt = BlobPutReceipt {
            blob_ref: aos_air_types::HashRef::new(stored.to_hex())?,
            edge_ref: aos_air_types::HashRef::new(edge_stored.to_hex())?,
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

#[cfg(test)]
mod tests {
    use super::*;
    use aos_effects::EffectKind;
    use aos_store::MemStore;

    fn blob_intent(params: BlobPutParams) -> EffectIntent {
        EffectIntent::from_raw_params(
            EffectKind::new(aos_effects::EffectKind::BLOB_PUT),
            "cap_blob",
            serde_cbor::to_vec(&params).expect("encode params"),
            [0u8; 32],
        )
        .expect("intent")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn blob_put_omitted_refs_defaults_to_leaf_and_edge_ref_is_stable() {
        let store = Arc::new(MemStore::default());
        let adapter = BlobPutAdapter::new(store.clone());
        let bytes = b"hello world".to_vec();
        let blob_ref = aos_air_types::HashRef::new(Hash::of_bytes(&bytes).to_hex()).unwrap();

        let params = BlobPutParams {
            bytes,
            blob_ref: Some(blob_ref),
            refs: None,
        };
        let intent = blob_intent(params.clone());
        let receipt_a = adapter.execute(&intent).await.expect("receipt A");
        let payload_a: BlobPutReceipt = serde_cbor::from_slice(&receipt_a.payload_cbor).unwrap();
        assert_eq!(receipt_a.status, ReceiptStatus::Ok);

        let receipt_b = adapter
            .execute(&blob_intent(params))
            .await
            .expect("receipt B");
        let payload_b: BlobPutReceipt = serde_cbor::from_slice(&receipt_b.payload_cbor).unwrap();
        assert_eq!(receipt_b.status, ReceiptStatus::Ok);
        assert_eq!(payload_a.edge_ref, payload_b.edge_ref);

        let edge_hash = Hash::from_hex_str(payload_a.edge_ref.as_str()).expect("edge hash");
        let edge_bytes = store.get_blob(edge_hash).expect("edge blob");
        let edge: BlobEdge = serde_cbor::from_slice(&edge_bytes).expect("decode edge");
        assert!(edge.refs.is_empty(), "omitted refs must normalize to leaf");
    }
}
