use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{
    BlobGetParams, BlobGetReceipt, BlobPutParams, BlobPutReceipt, EffectContinuationRef,
    EffectReceiptEnvelope, ObservedEffect, PendingEffect,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptOutcome<T> {
    Ok(T),
    Failed,
    InvalidPayload,
}

impl<T> ReceiptOutcome<T> {
    pub fn as_ok(&self) -> Option<&T> {
        match self {
            Self::Ok(value) => Some(value),
            Self::Failed | Self::InvalidPayload => None,
        }
    }

    pub fn ok(self) -> Option<T> {
        match self {
            Self::Ok(value) => Some(value),
            Self::Failed | Self::InvalidPayload => None,
        }
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed)
    }

    pub fn is_invalid_payload(&self) -> bool {
        matches!(self, Self::InvalidPayload)
    }
}

pub fn decode_receipt_outcome<T: DeserializeOwned>(
    envelope: &EffectReceiptEnvelope,
) -> ReceiptOutcome<T> {
    if envelope.status != "ok" {
        return ReceiptOutcome::Failed;
    }

    match envelope.decode_receipt_payload() {
        Ok(value) => ReceiptOutcome::Ok(value),
        Err(_) => ReceiptOutcome::InvalidPayload,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedPendingBegin {
    pub pending: PendingEffect,
    pub should_emit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(bound(serialize = "W: Serialize", deserialize = "W: Deserialize<'de>"))]
pub struct SharedPendingEffect<W> {
    pub pending: PendingEffect,
    pub waiters: Vec<W>,
}

impl<W> SharedPendingEffect<W> {
    pub fn new(pending: PendingEffect, waiter: W) -> Self {
        Self {
            pending,
            waiters: vec![waiter],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
#[serde(bound(serialize = "W: Serialize", deserialize = "W: Deserialize<'de>"))]
pub struct SharedPendingEffects<W> {
    by_params_hash: BTreeMap<String, SharedPendingEffect<W>>,
}

impl<W> Default for SharedPendingEffects<W> {
    fn default() -> Self {
        Self {
            by_params_hash: BTreeMap::new(),
        }
    }
}

impl<W> SharedPendingEffects<W> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.by_params_hash.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_params_hash.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &SharedPendingEffect<W>)> {
        self.by_params_hash.iter()
    }

    pub fn values(&self) -> impl Iterator<Item = &SharedPendingEffect<W>> {
        self.by_params_hash.values()
    }

    pub fn clear(&mut self) {
        self.by_params_hash.clear();
    }

    pub fn get(&self, params_hash: &str) -> Option<&SharedPendingEffect<W>> {
        self.by_params_hash.get(params_hash)
    }

    pub fn contains(&self, params_hash: &str) -> bool {
        self.by_params_hash.contains_key(params_hash)
    }

    pub fn begin<T: Serialize>(
        &mut self,
        effect: impl Into<String>,
        params: &T,
        emitted_at_ns: u64,
        waiter: W,
    ) -> Result<SharedPendingBegin, serde_cbor::Error> {
        let pending = PendingEffect::from_params(effect, params, emitted_at_ns)?;
        Ok(self.attach(pending, waiter))
    }

    pub fn attach(&mut self, pending: PendingEffect, waiter: W) -> SharedPendingBegin {
        if let Some(shared) = self.by_params_hash.get_mut(&pending.params_hash) {
            shared.waiters.push(waiter);
            return SharedPendingBegin {
                pending: shared.pending.clone(),
                should_emit: false,
            };
        }

        let params_hash = pending.params_hash.clone();
        self.by_params_hash.insert(
            params_hash,
            SharedPendingEffect::new(pending.clone(), waiter),
        );
        SharedPendingBegin {
            pending,
            should_emit: true,
        }
    }

    pub fn observe<'a>(
        &mut self,
        continuation: EffectContinuationRef<'a>,
    ) -> Option<SharedPendingEffectMatch<'a, W>>
    where
        W: Clone,
    {
        let key = self.lookup_key(continuation)?;
        let shared = self.by_params_hash.get_mut(key.as_str())?;
        let observed = shared.pending.observe(continuation)?;
        Some(SharedPendingEffectMatch {
            pending: shared.pending.clone(),
            waiters: shared.waiters.clone(),
            observed,
        })
    }

    pub fn settle<'a>(
        &mut self,
        continuation: EffectContinuationRef<'a>,
    ) -> Option<SharedPendingEffectMatch<'a, W>> {
        if !continuation.is_terminal() {
            return None;
        }

        let key = self.lookup_key(continuation)?;
        let mut shared = self.by_params_hash.remove(key.as_str())?;
        let observed = shared.pending.observe(continuation)?;
        Some(SharedPendingEffectMatch {
            pending: shared.pending,
            waiters: shared.waiters,
            observed,
        })
    }

    pub fn settle_receipt<'a, R: DeserializeOwned>(
        &mut self,
        envelope: &'a EffectReceiptEnvelope,
    ) -> Option<SharedSettledReceipt<'a, W, R>> {
        let matched = self.settle(envelope.into())?;
        Some(SharedSettledReceipt {
            pending: matched.pending,
            waiters: matched.waiters,
            observed: envelope,
            receipt: decode_receipt_outcome(envelope),
        })
    }

    fn lookup_key(&self, continuation: EffectContinuationRef<'_>) -> Option<String> {
        if let Some((params_hash, _)) = self.by_params_hash.iter().find(|(_, shared)| {
            shared.pending.intent_id.as_deref() == Some(continuation.intent_id())
        }) {
            return Some(params_hash.clone());
        }

        continuation
            .issuer_ref()
            .and_then(|issuer_ref| {
                self.by_params_hash
                    .iter()
                    .find(|(_, shared)| shared.pending.issuer_ref.as_deref() == Some(issuer_ref))
                    .map(|(params_hash, _)| params_hash.clone())
            })
            .or_else(|| {
                continuation.params_hash().and_then(|params_hash| {
                    self.by_params_hash
                        .get(params_hash)
                        .filter(|shared| shared.pending.effect == continuation.effect())
                        .map(|_| params_hash.to_string())
                })
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedPendingEffectMatch<'a, W> {
    pub pending: PendingEffect,
    pub waiters: Vec<W>,
    pub observed: ObservedEffect<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedSettledReceipt<'a, W, R> {
    pub pending: PendingEffect,
    pub waiters: Vec<W>,
    pub observed: &'a EffectReceiptEnvelope,
    pub receipt: ReceiptOutcome<R>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
#[serde(bound(serialize = "W: Serialize", deserialize = "W: Deserialize<'de>"))]
pub struct SharedBlobGets<W> {
    pending: SharedPendingEffects<W>,
}

impl<W> Default for SharedBlobGets<W> {
    fn default() -> Self {
        Self {
            pending: SharedPendingEffects::new(),
        }
    }
}

impl<W> SharedBlobGets<W> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn values(&self) -> impl Iterator<Item = &SharedPendingEffect<W>> {
        self.pending.values()
    }

    pub fn contains(&self, params_hash: &str) -> bool {
        self.pending.contains(params_hash)
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }

    pub fn begin(
        &mut self,
        params: &BlobGetParams,
        emitted_at_ns: u64,
        waiter: W,
    ) -> Result<SharedPendingBegin, serde_cbor::Error> {
        self.pending
            .begin("sys/blob.get@1", params, emitted_at_ns, waiter)
    }

    pub fn attach(&mut self, pending: PendingEffect, waiter: W) -> SharedPendingBegin {
        self.pending.attach(pending, waiter)
    }

    pub fn settle<'a>(
        &mut self,
        envelope: &'a EffectReceiptEnvelope,
    ) -> Option<SharedSettledReceipt<'a, W, BlobGetReceipt>> {
        self.pending.settle_receipt(envelope)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
#[serde(bound(serialize = "W: Serialize", deserialize = "W: Deserialize<'de>"))]
pub struct SharedBlobPuts<W> {
    pending: SharedPendingEffects<W>,
}

impl<W> Default for SharedBlobPuts<W> {
    fn default() -> Self {
        Self {
            pending: SharedPendingEffects::new(),
        }
    }
}

impl<W> SharedBlobPuts<W> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn values(&self) -> impl Iterator<Item = &SharedPendingEffect<W>> {
        self.pending.values()
    }

    pub fn contains(&self, params_hash: &str) -> bool {
        self.pending.contains(params_hash)
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }

    pub fn begin(
        &mut self,
        params: &BlobPutParams,
        emitted_at_ns: u64,
        waiter: W,
    ) -> Result<SharedPendingBegin, serde_cbor::Error> {
        self.pending
            .begin("sys/blob.put@1", params, emitted_at_ns, waiter)
    }

    pub fn attach(&mut self, pending: PendingEffect, waiter: W) -> SharedPendingBegin {
        self.pending.attach(pending, waiter)
    }

    pub fn settle<'a>(
        &mut self,
        envelope: &'a EffectReceiptEnvelope,
    ) -> Option<SharedSettledReceipt<'a, W, BlobPutReceipt>> {
        self.pending.settle_receipt(envelope)
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;

    use serde::{Deserialize, Serialize};

    use crate::{EffectReceiptEnvelope, PendingEffect, hash_bytes};

    use super::{ReceiptOutcome, SharedBlobGets, SharedPendingEffects};

    fn fake_hash(byte: char) -> String {
        hash_bytes(&[byte as u8])
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    struct DummyReceipt {
        status: i32,
    }

    #[test]
    fn shared_pending_effects_fan_out_terminal_receipt() {
        let mut pending = SharedPendingEffects::new();
        let begin1 = pending
            .begin("sys/blob.get@1", &vec!["same"], 7, "w1")
            .unwrap();
        let begin2 = pending
            .begin("sys/blob.get@1", &vec!["same"], 8, "w2")
            .unwrap();

        assert!(begin1.should_emit);
        assert!(!begin2.should_emit);
        assert_eq!(pending.len(), 1);

        let receipt = EffectReceiptEnvelope {
            intent_id: fake_hash('i'),
            effect: "sys/blob.get@1".into(),
            params_hash: Some(begin1.pending.params_hash.clone()),
            receipt_payload: serde_cbor::to_vec(&DummyReceipt { status: 200 }).unwrap(),
            status: "ok".into(),
            ..EffectReceiptEnvelope::default()
        };

        let settled = pending.settle((&receipt).into()).expect("settled");
        assert_eq!(settled.waiters, vec!["w1", "w2"]);
        assert!(pending.is_empty());
    }

    #[test]
    fn shared_blob_gets_decode_terminal_receipt() {
        let mut pending = SharedBlobGets::new();
        let params = crate::BlobGetParams {
            blob_ref: fake_hash('b').parse().unwrap(),
        };
        let begin = pending.begin(&params, 7, "w1").unwrap();

        let receipt = EffectReceiptEnvelope {
            intent_id: fake_hash('i'),
            effect: "sys/blob.get@1".into(),
            params_hash: Some(begin.pending.params_hash.clone()),
            receipt_payload: serde_cbor::to_vec(&crate::BlobGetReceipt {
                blob_ref: params.blob_ref.clone(),
                size: 4,
                bytes: vec![1, 2, 3, 4],
            })
            .unwrap(),
            status: "ok".into(),
            ..EffectReceiptEnvelope::default()
        };

        let settled = pending.settle(&receipt).expect("settled");
        assert!(matches!(settled.receipt, ReceiptOutcome::Ok(_)));
        assert!(pending.is_empty());
    }

    #[test]
    fn shared_pending_effect_attach_preserves_pending_handle() {
        let mut pending = SharedPendingEffects::new();
        let tracked = PendingEffect::new("sys/blob.put@1", fake_hash('p'), 9);
        let begin = pending.attach(tracked.clone(), "waiter");

        assert!(begin.should_emit);
        assert_eq!(begin.pending, tracked);
    }
}
