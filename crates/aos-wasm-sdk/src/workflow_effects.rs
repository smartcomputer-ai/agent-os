use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod serde_bytes_opt {
    use alloc::vec::Vec;
    use serde::{Deserialize, Deserializer, Serializer};
    use serde_bytes::{ByteBuf, Bytes};

    pub fn serialize<S>(value: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(bytes) => serializer.serialize_some(Bytes::new(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<ByteBuf>::deserialize(deserializer).map(|opt| opt.map(|buf| buf.into_vec()))
    }
}

mod serde_bytes_vec {
    use alloc::vec::Vec;
    use serde::{Deserialize, Deserializer, Serializer};
    use serde_bytes::ByteBuf;

    pub fn serialize<S>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(value)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        ByteBuf::deserialize(deserializer).map(ByteBuf::into_vec)
    }
}

/// Generic workflow receipt envelope delivered to workflows for external effects.
///
/// The `receipt_payload` field contains the canonical CBOR payload validated against
/// the effect's declared `receipt_schema` in the kernel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EffectReceiptEnvelope {
    pub origin_module_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub origin_instance_key: Option<Vec<u8>>,
    pub intent_id: String,
    pub effect_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_ref: Option<String>,
    #[serde(with = "serde_bytes_vec")]
    pub receipt_payload: Vec<u8>,
    pub status: String,
    pub emitted_at_seq: u64,
    pub adapter_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cents: Option<u64>,
    #[serde(with = "serde_bytes_vec")]
    pub signature: Vec<u8>,
}

impl EffectReceiptEnvelope {
    /// Decode the embedded receipt payload into a typed struct.
    pub fn decode_receipt_payload<T: DeserializeOwned>(&self) -> Result<T, serde_cbor::Error> {
        serde_cbor::from_slice(&self.receipt_payload)
    }
}

/// Generic workflow rejection envelope emitted when a receipt cannot be normalized
/// or delivered according to the workflow's event contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EffectReceiptRejected {
    pub origin_module_id: String,
    #[serde(
        default,
        with = "serde_bytes_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub origin_instance_key: Option<Vec<u8>>,
    pub intent_id: String,
    pub effect_kind: String,
    pub params_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_ref: Option<String>,
    pub adapter_id: String,
    pub status: String,
    pub error_code: String,
    pub error_message: String,
    pub payload_hash: String,
    pub payload_size: u64,
    pub emitted_at_seq: u64,
}

/// Generic workflow stream frame envelope delivered while a long-lived effect is open.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EffectStreamFrameEnvelope {
    pub origin_module_id: String,
    #[serde(
        default,
        with = "serde_bytes_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub origin_instance_key: Option<Vec<u8>>,
    pub intent_id: String,
    pub effect_kind: String,
    pub params_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_ref: Option<String>,
    pub emitted_at_seq: u64,
    pub seq: u64,
    pub kind: String,
    #[serde(with = "serde_bytes_vec")]
    pub payload: Vec<u8>,
    pub payload_ref: Option<String>,
    pub adapter_id: String,
    #[serde(with = "serde_bytes_vec")]
    pub signature: Vec<u8>,
}

impl EffectStreamFrameEnvelope {
    /// Decode the embedded stream payload into a typed struct.
    pub fn decode_payload<T: DeserializeOwned>(&self) -> Result<T, serde_cbor::Error> {
        serde_cbor::from_slice(&self.payload)
    }
}

/// Borrowed view over workflow continuation envelopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectContinuationRef<'a> {
    Receipt(&'a EffectReceiptEnvelope),
    Rejected(&'a EffectReceiptRejected),
    Stream(&'a EffectStreamFrameEnvelope),
}

impl<'a> EffectContinuationRef<'a> {
    pub fn intent_id(self) -> &'a str {
        match self {
            Self::Receipt(value) => value.intent_id.as_str(),
            Self::Rejected(value) => value.intent_id.as_str(),
            Self::Stream(value) => value.intent_id.as_str(),
        }
    }

    pub fn effect_kind(self) -> &'a str {
        match self {
            Self::Receipt(value) => value.effect_kind.as_str(),
            Self::Rejected(value) => value.effect_kind.as_str(),
            Self::Stream(value) => value.effect_kind.as_str(),
        }
    }

    pub fn params_hash(self) -> Option<&'a str> {
        match self {
            Self::Receipt(value) => value.params_hash.as_deref(),
            Self::Rejected(value) => value.params_hash.as_deref(),
            Self::Stream(value) => value.params_hash.as_deref(),
        }
    }

    pub fn issuer_ref(self) -> Option<&'a str> {
        match self {
            Self::Receipt(value) => value.issuer_ref.as_deref(),
            Self::Rejected(value) => value.issuer_ref.as_deref(),
            Self::Stream(value) => value.issuer_ref.as_deref(),
        }
    }

    pub fn emitted_at_seq(self) -> u64 {
        match self {
            Self::Receipt(value) => value.emitted_at_seq,
            Self::Rejected(value) => value.emitted_at_seq,
            Self::Stream(value) => value.emitted_at_seq,
        }
    }

    pub fn adapter_id(self) -> &'a str {
        match self {
            Self::Receipt(value) => value.adapter_id.as_str(),
            Self::Rejected(value) => value.adapter_id.as_str(),
            Self::Stream(value) => value.adapter_id.as_str(),
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Stream(_))
    }
}

impl<'a> From<&'a EffectReceiptEnvelope> for EffectContinuationRef<'a> {
    fn from(value: &'a EffectReceiptEnvelope) -> Self {
        Self::Receipt(value)
    }
}

impl<'a> From<&'a EffectReceiptRejected> for EffectContinuationRef<'a> {
    fn from(value: &'a EffectReceiptRejected) -> Self {
        Self::Rejected(value)
    }
}

impl<'a> From<&'a EffectStreamFrameEnvelope> for EffectContinuationRef<'a> {
    fn from(value: &'a EffectStreamFrameEnvelope) -> Self {
        Self::Stream(value)
    }
}

/// Owned workflow continuation envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectContinuation {
    Receipt(EffectReceiptEnvelope),
    Rejected(EffectReceiptRejected),
    Stream(EffectStreamFrameEnvelope),
}

impl EffectContinuation {
    pub fn as_ref(&self) -> EffectContinuationRef<'_> {
        match self {
            Self::Receipt(value) => value.into(),
            Self::Rejected(value) => value.into(),
            Self::Stream(value) => value.into(),
        }
    }
}

impl From<EffectReceiptEnvelope> for EffectContinuation {
    fn from(value: EffectReceiptEnvelope) -> Self {
        Self::Receipt(value)
    }
}

impl From<EffectReceiptRejected> for EffectContinuation {
    fn from(value: EffectReceiptRejected) -> Self {
        Self::Rejected(value)
    }
}

impl From<EffectStreamFrameEnvelope> for EffectContinuation {
    fn from(value: EffectStreamFrameEnvelope) -> Self {
        Self::Stream(value)
    }
}

/// Durable workflow-side handle for a pending effect intent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PendingEffect {
    pub effect_kind: String,
    pub params_hash: String,
    pub intent_id: Option<String>,
    pub issuer_ref: Option<String>,
    pub cap_slot: Option<String>,
    pub emitted_at_ns: u64,
    pub last_stream_seq: u64,
}

impl PendingEffect {
    pub fn new(
        effect_kind: impl Into<String>,
        params_hash: impl Into<String>,
        cap_slot: Option<String>,
        emitted_at_ns: u64,
    ) -> Self {
        Self {
            effect_kind: effect_kind.into(),
            params_hash: params_hash.into(),
            intent_id: None,
            issuer_ref: None,
            cap_slot,
            emitted_at_ns,
            last_stream_seq: 0,
        }
    }

    pub fn with_issuer_ref(mut self, issuer_ref: impl Into<String>) -> Self {
        self.issuer_ref = Some(issuer_ref.into());
        self
    }

    pub fn with_issuer_ref_opt(mut self, issuer_ref: Option<String>) -> Self {
        self.issuer_ref = issuer_ref;
        self
    }

    pub fn from_params<T: Serialize>(
        effect_kind: impl Into<String>,
        params: &T,
        cap_slot: Option<String>,
        emitted_at_ns: u64,
    ) -> Result<Self, serde_cbor::Error> {
        Self::from_params_with_issuer_ref(effect_kind, params, cap_slot, emitted_at_ns, None)
    }

    pub fn from_params_with_issuer_ref<T: Serialize>(
        effect_kind: impl Into<String>,
        params: &T,
        cap_slot: Option<String>,
        emitted_at_ns: u64,
        issuer_ref: Option<String>,
    ) -> Result<Self, serde_cbor::Error> {
        Ok(Self::new(
            effect_kind,
            effect_params_hash(params)?,
            cap_slot,
            emitted_at_ns,
        )
        .with_issuer_ref_opt(issuer_ref))
    }

    pub fn matches(&self, continuation: EffectContinuationRef<'_>) -> bool {
        if self.effect_kind != continuation.effect_kind() {
            return false;
        }

        if let Some(intent_id) = self.intent_id.as_deref() {
            return intent_id == continuation.intent_id();
        }

        if let Some(issuer_ref) = self.issuer_ref.as_deref() {
            return continuation
                .issuer_ref()
                .is_some_and(|value| value == issuer_ref);
        }

        continuation
            .params_hash()
            .is_some_and(|params_hash| params_hash == self.params_hash)
    }

    pub fn observe<'a>(
        &mut self,
        continuation: EffectContinuationRef<'a>,
    ) -> Option<ObservedEffect<'a>> {
        if !self.matches(continuation) {
            return None;
        }

        if self.intent_id.is_none() {
            self.intent_id = Some(continuation.intent_id().to_string());
        }

        if let EffectContinuationRef::Stream(frame) = continuation {
            self.last_stream_seq = self.last_stream_seq.max(frame.seq);
            return Some(ObservedEffect::Stream(frame));
        }

        match continuation {
            EffectContinuationRef::Receipt(receipt) => Some(ObservedEffect::Settled(receipt)),
            EffectContinuationRef::Rejected(rejected) => Some(ObservedEffect::Rejected(rejected)),
            EffectContinuationRef::Stream(_) => None,
        }
    }
}

/// Workflow-local storage for in-flight effect handles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(transparent)]
pub struct PendingEffects {
    by_params_hash: BTreeMap<String, PendingEffect>,
}

impl PendingEffects {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.by_params_hash.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_params_hash.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &PendingEffect)> {
        self.by_params_hash.iter()
    }

    pub fn values(&self) -> impl Iterator<Item = &PendingEffect> {
        self.by_params_hash.values()
    }

    pub fn clear(&mut self) {
        self.by_params_hash.clear();
    }

    pub fn contains_effect_kind(&self, effect_kind: &str) -> bool {
        self.values()
            .any(|pending| pending.effect_kind == effect_kind)
    }

    pub fn insert(&mut self, pending: PendingEffect) -> Option<PendingEffect> {
        self.by_params_hash
            .insert(pending.params_hash.clone(), pending)
    }

    pub fn begin<T: Serialize>(
        &mut self,
        effect_kind: impl Into<String>,
        params: &T,
        cap_slot: Option<String>,
        emitted_at_ns: u64,
    ) -> Result<PendingEffect, serde_cbor::Error> {
        self.begin_with_issuer_ref(effect_kind, params, cap_slot, emitted_at_ns, None)
    }

    pub fn begin_with_issuer_ref<T: Serialize>(
        &mut self,
        effect_kind: impl Into<String>,
        params: &T,
        cap_slot: Option<String>,
        emitted_at_ns: u64,
        issuer_ref: Option<String>,
    ) -> Result<PendingEffect, serde_cbor::Error> {
        let pending = PendingEffect::from_params_with_issuer_ref(
            effect_kind,
            params,
            cap_slot,
            emitted_at_ns,
            issuer_ref,
        )?;
        self.insert(pending.clone());
        Ok(pending)
    }

    pub fn get(&self, params_hash: &str) -> Option<&PendingEffect> {
        self.by_params_hash.get(params_hash)
    }

    pub fn remove(&mut self, params_hash: &str) -> Option<PendingEffect> {
        self.by_params_hash.remove(params_hash)
    }

    pub fn observe<'a>(
        &mut self,
        continuation: EffectContinuationRef<'a>,
    ) -> Option<PendingEffectMatch<'a>> {
        let key = self.lookup_key(continuation)?;
        let pending = self.by_params_hash.get_mut(key.as_str())?;
        let observed = pending.observe(continuation)?;
        Some(PendingEffectMatch {
            pending: pending.clone(),
            observed,
        })
    }

    pub fn settle<'a>(
        &mut self,
        continuation: EffectContinuationRef<'a>,
    ) -> Option<PendingEffectMatch<'a>> {
        if !continuation.is_terminal() {
            return None;
        }

        let key = self.lookup_key(continuation)?;
        let mut pending = self.by_params_hash.remove(key.as_str())?;
        let observed = pending.observe(continuation)?;
        Some(PendingEffectMatch { pending, observed })
    }

    fn lookup_key(&self, continuation: EffectContinuationRef<'_>) -> Option<String> {
        if let Some((params_hash, _)) = self
            .by_params_hash
            .iter()
            .find(|(_, pending)| pending.intent_id.as_deref() == Some(continuation.intent_id()))
        {
            return Some((*params_hash).clone());
        }

        if let Some(issuer_ref) = continuation.issuer_ref()
            && let Some((params_hash, _)) = self
                .by_params_hash
                .iter()
                .find(|(_, pending)| pending.issuer_ref.as_deref() == Some(issuer_ref))
        {
            return Some((*params_hash).clone());
        }

        continuation.params_hash().and_then(|params_hash| {
            self.by_params_hash
                .get(params_hash)
                .filter(|pending| pending.effect_kind == continuation.effect_kind())
                .map(|_| params_hash.to_string())
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingEffectLookupError {
    AmbiguousIssuerRef {
        issuer_ref: String,
        matches: usize,
    },
    AmbiguousParamsHash {
        effect_kind: String,
        params_hash: String,
        matches: usize,
    },
}

impl core::fmt::Display for PendingEffectLookupError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AmbiguousIssuerRef {
                issuer_ref,
                matches,
            } => write!(
                f,
                "multiple pending effects match issuer_ref {issuer_ref}: {matches}"
            ),
            Self::AmbiguousParamsHash {
                effect_kind,
                params_hash,
                matches,
            } => write!(
                f,
                "multiple pending effects match {effect_kind} with params_hash {params_hash}: {matches}"
            ),
        }
    }
}

impl core::error::Error for PendingEffectLookupError {}

/// Workflow-local storage for in-flight effect handles keyed by workflow state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
#[serde(bound(serialize = "K: Serialize", deserialize = "K: Ord + Deserialize<'de>"))]
pub struct PendingEffectSet<K> {
    by_key: BTreeMap<K, PendingEffect>,
}

impl<K> Default for PendingEffectSet<K> {
    fn default() -> Self {
        Self {
            by_key: BTreeMap::new(),
        }
    }
}

impl<K> PendingEffectSet<K>
where
    K: Ord + Clone,
{
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &PendingEffect)> {
        self.by_key.iter()
    }

    pub fn values(&self) -> impl Iterator<Item = &PendingEffect> {
        self.by_key.values()
    }

    pub fn clear(&mut self) {
        self.by_key.clear();
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.by_key.contains_key(key)
    }

    pub fn insert(&mut self, key: K, pending: PendingEffect) -> Option<PendingEffect> {
        self.by_key.insert(key, pending)
    }

    pub fn begin<T: Serialize>(
        &mut self,
        key: K,
        effect_kind: impl Into<String>,
        params: &T,
        cap_slot: Option<String>,
        emitted_at_ns: u64,
    ) -> Result<PendingEffect, serde_cbor::Error> {
        self.begin_with_issuer_ref(key, effect_kind, params, cap_slot, emitted_at_ns, None)
    }

    pub fn begin_with_issuer_ref<T: Serialize>(
        &mut self,
        key: K,
        effect_kind: impl Into<String>,
        params: &T,
        cap_slot: Option<String>,
        emitted_at_ns: u64,
        issuer_ref: Option<String>,
    ) -> Result<PendingEffect, serde_cbor::Error> {
        let pending = PendingEffect::from_params_with_issuer_ref(
            effect_kind,
            params,
            cap_slot,
            emitted_at_ns,
            issuer_ref,
        )?;
        self.insert(key, pending.clone());
        Ok(pending)
    }

    pub fn get(&self, key: &K) -> Option<&PendingEffect> {
        self.by_key.get(key)
    }

    pub fn remove(&mut self, key: &K) -> Option<PendingEffect> {
        self.by_key.remove(key)
    }

    pub fn observe<'a>(
        &mut self,
        continuation: EffectContinuationRef<'a>,
    ) -> Result<Option<PendingEffectSetMatch<'a, K>>, PendingEffectLookupError> {
        let Some(key) = self.lookup_key(continuation)? else {
            return Ok(None);
        };
        let pending = self.by_key.get_mut(&key).expect("matched pending effect");
        let observed = pending.observe(continuation).expect("matched continuation");
        Ok(Some(PendingEffectSetMatch {
            key,
            pending: pending.clone(),
            observed,
        }))
    }

    pub fn settle<'a>(
        &mut self,
        continuation: EffectContinuationRef<'a>,
    ) -> Result<Option<PendingEffectSetMatch<'a, K>>, PendingEffectLookupError> {
        if !continuation.is_terminal() {
            return Ok(None);
        }

        let Some(key) = self.lookup_key(continuation)? else {
            return Ok(None);
        };
        let mut pending = self.by_key.remove(&key).expect("matched pending effect");
        let observed = pending.observe(continuation).expect("matched continuation");
        Ok(Some(PendingEffectSetMatch {
            key,
            pending,
            observed,
        }))
    }

    fn lookup_key(
        &self,
        continuation: EffectContinuationRef<'_>,
    ) -> Result<Option<K>, PendingEffectLookupError> {
        if let Some((key, _)) = self
            .by_key
            .iter()
            .find(|(_, pending)| pending.intent_id.as_deref() == Some(continuation.intent_id()))
        {
            return Ok(Some(key.clone()));
        }

        if let Some(issuer_ref) = continuation.issuer_ref() {
            let matches = self
                .by_key
                .iter()
                .filter(|(_, pending)| pending.issuer_ref.as_deref() == Some(issuer_ref))
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            return match matches.len() {
                0 => Ok(None),
                1 => Ok(matches.into_iter().next()),
                count => Err(PendingEffectLookupError::AmbiguousIssuerRef {
                    issuer_ref: issuer_ref.to_string(),
                    matches: count,
                }),
            };
        }

        let Some(params_hash) = continuation.params_hash() else {
            return Ok(None);
        };
        let matches = self
            .by_key
            .iter()
            .filter(|(_, pending)| {
                pending.effect_kind == continuation.effect_kind()
                    && pending.params_hash == params_hash
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.into_iter().next()),
            count => Err(PendingEffectLookupError::AmbiguousParamsHash {
                effect_kind: continuation.effect_kind().to_string(),
                params_hash: params_hash.to_string(),
                matches: count,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingEffectSetMatch<'a, K> {
    pub key: K,
    pub pending: PendingEffect,
    pub observed: ObservedEffect<'a>,
}

/// Durable completion state for one `await_all` group of keyed effects.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(bound(serialize = "K: Serialize", deserialize = "K: Ord + Deserialize<'de>"))]
pub struct PendingBatchGroup<K> {
    pub keys: Vec<K>,
    pub terminal_keys: BTreeSet<K>,
}

impl<K> Default for PendingBatchGroup<K> {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            terminal_keys: BTreeSet::new(),
        }
    }
}

impl<K> PendingBatchGroup<K>
where
    K: Ord + Clone,
{
    pub fn new(keys: Vec<K>) -> Self {
        Self {
            keys,
            terminal_keys: BTreeSet::new(),
        }
    }

    pub fn contains(&self, key: &K) -> bool {
        self.keys.iter().any(|candidate| candidate == key)
    }

    pub fn mark_terminal(&mut self, key: &K) -> bool {
        if !self.contains(key) {
            return false;
        }
        self.terminal_keys.insert(key.clone())
    }

    pub fn is_complete(&self) -> bool {
        self.keys.iter().all(|key| self.terminal_keys.contains(key))
    }
}

/// Durable sequential batch state built from `await_all` groups of keyed effects.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(bound(serialize = "K: Serialize", deserialize = "K: Ord + Deserialize<'de>"))]
pub struct PendingBatch<K> {
    pub groups: Vec<PendingBatchGroup<K>>,
    pub next_group_index: u64,
}

impl<K> Default for PendingBatch<K> {
    fn default() -> Self {
        Self {
            groups: Vec::new(),
            next_group_index: 0,
        }
    }
}

impl<K> PendingBatch<K>
where
    K: Ord + Clone,
{
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_groups(groups: Vec<Vec<K>>) -> Self {
        Self {
            groups: groups.into_iter().map(PendingBatchGroup::new).collect(),
            next_group_index: 0,
        }
    }

    pub fn current_group_index(&self) -> Option<usize> {
        let idx = self.next_group_index as usize;
        (idx < self.groups.len()).then_some(idx)
    }

    pub fn current_group(&self) -> Option<&PendingBatchGroup<K>> {
        self.current_group_index()
            .and_then(|idx| self.groups.get(idx))
    }

    pub fn current_group_keys(&self) -> Option<&[K]> {
        self.current_group().map(|group| group.keys.as_slice())
    }

    pub fn advance(&mut self) -> bool {
        if self.current_group_index().is_none() {
            return false;
        }
        self.next_group_index = self.next_group_index.saturating_add(1);
        true
    }

    pub fn advance_completed(&mut self) -> usize {
        let mut advanced = 0usize;
        while self
            .current_group()
            .is_some_and(PendingBatchGroup::is_complete)
        {
            self.next_group_index = self.next_group_index.saturating_add(1);
            advanced = advanced.saturating_add(1);
        }
        advanced
    }

    pub fn is_complete(&self) -> bool {
        self.current_group_index().is_none()
    }

    pub fn group_index_of(&self, key: &K) -> Option<usize> {
        self.groups.iter().position(|group| group.contains(key))
    }

    pub fn rewind_to_group_containing(&mut self, key: &K) -> bool {
        let Some(idx) = self.group_index_of(key) else {
            return false;
        };
        if idx >= self.next_group_index as usize {
            return false;
        }
        self.next_group_index = idx as u64;
        true
    }

    pub fn mark_terminal(&mut self, key: &K) -> bool {
        self.groups
            .iter_mut()
            .find(|group| group.contains(key))
            .is_some_and(|group| group.mark_terminal(key))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingEffectMatch<'a> {
    pub pending: PendingEffect,
    pub observed: ObservedEffect<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservedEffect<'a> {
    Stream(&'a EffectStreamFrameEnvelope),
    Settled(&'a EffectReceiptEnvelope),
    Rejected(&'a EffectReceiptRejected),
}

impl<'a> ObservedEffect<'a> {
    pub fn continuation(self) -> EffectContinuationRef<'a> {
        match self {
            Self::Stream(value) => value.into(),
            Self::Settled(value) => value.into(),
            Self::Rejected(value) => value.into(),
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Stream(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedEffectParams {
    pub cbor: Vec<u8>,
    pub params_hash: String,
}

pub fn encode_effect_params<T: Serialize>(
    params: &T,
) -> Result<EncodedEffectParams, serde_cbor::Error> {
    let cbor = serde_cbor::to_vec(params)?;
    let params_hash = hash_bytes(&cbor);
    Ok(EncodedEffectParams { cbor, params_hash })
}

pub fn effect_params_hash<T: Serialize>(params: &T) -> Result<String, serde_cbor::Error> {
    Ok(encode_effect_params(params)?.params_hash)
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::from("sha256:");
    for byte in digest {
        out.push(nibble_to_hex(byte >> 4));
        out.push(nibble_to_hex(byte & 0x0f));
    }
    out
}

fn nibble_to_hex(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'a' + (value - 10)) as char,
    }
}

pub use aos_effect_types::*;

pub struct SysEffects<'a, 'ctx, S, A> {
    effects: &'a mut crate::Effects<'ctx, S, A>,
}

impl<'a, 'ctx, S, A> SysEffects<'a, 'ctx, S, A> {
    pub(crate) fn new(effects: &'a mut crate::Effects<'ctx, S, A>) -> Self {
        Self { effects }
    }
}

macro_rules! define_sys_effect_helpers {
    ($(($emit:ident, $emit_tracked:ident, $kind:expr, $params:ty)),+ $(,)?) => {
        impl<'a, 'ctx, S, A> SysEffects<'a, 'ctx, S, A> {
            $(
                pub fn $emit(&mut self, params: &$params, cap_slot: &str)
                where
                    $params: Serialize,
                {
                    self.effects.emit_raw($kind, params, Some(cap_slot));
                }

                pub fn $emit_tracked(
                    &mut self,
                    pending: &mut PendingEffects,
                    params: &$params,
                    cap_slot: &str,
                ) -> PendingEffect
                where
                    $params: Serialize,
                {
                    self.effects.emit_tracked(pending, $kind, params, Some(cap_slot))
                }
            )+
        }
    };
}

define_sys_effect_helpers!(
    (
        http_request,
        http_request_tracked,
        "http.request",
        HttpRequestParams
    ),
    (blob_put, blob_put_tracked, "blob.put", BlobPutParams),
    (blob_get, blob_get_tracked, "blob.get", BlobGetParams),
    (timer_set, timer_set_tracked, "timer.set", TimerSetParams),
    (
        host_session_open,
        host_session_open_tracked,
        "host.session.open",
        HostSessionOpenParams
    ),
    (host_exec, host_exec_tracked, "host.exec", HostExecParams),
    (
        host_session_signal,
        host_session_signal_tracked,
        "host.session.signal",
        HostSessionSignalParams
    ),
    (
        host_fs_read_file,
        host_fs_read_file_tracked,
        "host.fs.read_file",
        HostFsReadFileParams
    ),
    (
        host_fs_write_file,
        host_fs_write_file_tracked,
        "host.fs.write_file",
        HostFsWriteFileParams
    ),
    (
        host_fs_edit_file,
        host_fs_edit_file_tracked,
        "host.fs.edit_file",
        HostFsEditFileParams
    ),
    (
        host_fs_apply_patch,
        host_fs_apply_patch_tracked,
        "host.fs.apply_patch",
        HostFsApplyPatchParams
    ),
    (
        host_fs_grep,
        host_fs_grep_tracked,
        "host.fs.grep",
        HostFsGrepParams
    ),
    (
        host_fs_glob,
        host_fs_glob_tracked,
        "host.fs.glob",
        HostFsGlobParams
    ),
    (
        host_fs_stat,
        host_fs_stat_tracked,
        "host.fs.stat",
        HostFsStatParams
    ),
    (
        host_fs_exists,
        host_fs_exists_tracked,
        "host.fs.exists",
        HostFsExistsParams
    ),
    (
        host_fs_list_dir,
        host_fs_list_dir_tracked,
        "host.fs.list_dir",
        HostFsListDirParams
    ),
    (
        llm_generate,
        llm_generate_tracked,
        "llm.generate",
        LlmGenerateParams
    ),
    (vault_put, vault_put_tracked, "vault.put", VaultPutParams),
    (
        vault_rotate,
        vault_rotate_tracked,
        "vault.rotate",
        VaultRotateParams
    ),
    (
        governance_propose,
        governance_propose_tracked,
        "governance.propose",
        GovProposeParams
    ),
    (
        governance_shadow,
        governance_shadow_tracked,
        "governance.shadow",
        GovShadowParams
    ),
    (
        governance_approve,
        governance_approve_tracked,
        "governance.approve",
        GovApproveParams
    ),
    (
        governance_apply,
        governance_apply_tracked,
        "governance.apply",
        GovApplyParams
    ),
    (
        workspace_resolve,
        workspace_resolve_tracked,
        "workspace.resolve",
        WorkspaceResolveParams
    ),
    (
        workspace_empty_root,
        workspace_empty_root_tracked,
        "workspace.empty_root",
        WorkspaceEmptyRootParams
    ),
    (
        workspace_list,
        workspace_list_tracked,
        "workspace.list",
        WorkspaceListParams
    ),
    (
        workspace_read_ref,
        workspace_read_ref_tracked,
        "workspace.read_ref",
        WorkspaceReadRefParams
    ),
    (
        workspace_read_bytes,
        workspace_read_bytes_tracked,
        "workspace.read_bytes",
        WorkspaceReadBytesParams
    ),
    (
        workspace_write_bytes,
        workspace_write_bytes_tracked,
        "workspace.write_bytes",
        WorkspaceWriteBytesParams
    ),
    (
        workspace_write_ref,
        workspace_write_ref_tracked,
        "workspace.write_ref",
        WorkspaceWriteRefParams
    ),
    (
        workspace_remove,
        workspace_remove_tracked,
        "workspace.remove",
        WorkspaceRemoveParams
    ),
    (
        workspace_diff,
        workspace_diff_tracked,
        "workspace.diff",
        WorkspaceDiffParams
    ),
    (
        workspace_annotations_get,
        workspace_annotations_get_tracked,
        "workspace.annotations_get",
        WorkspaceAnnotationsGetParams
    ),
    (
        workspace_annotations_set,
        workspace_annotations_set_tracked,
        "workspace.annotations_set",
        WorkspaceAnnotationsSetParams
    ),
    (
        introspect_manifest,
        introspect_manifest_tracked,
        "introspect.manifest",
        IntrospectManifestParams
    ),
    (
        introspect_workflow_state,
        introspect_workflow_state_tracked,
        "introspect.workflow_state",
        IntrospectWorkflowStateParams
    ),
    (
        introspect_journal_head,
        introspect_journal_head_tracked,
        "introspect.journal_head",
        IntrospectJournalHeadParams
    ),
    (
        introspect_list_cells,
        introspect_list_cells_tracked,
        "introspect.list_cells",
        IntrospectListCellsParams
    )
);

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn fake_hash(seed: char) -> String {
        let mut out = String::from("sha256:");
        for _ in 0..64 {
            out.push(seed);
        }
        out
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    struct DummyReceipt {
        status: i32,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    struct DummyStream {
        chunk: u64,
    }

    #[test]
    fn stream_frame_decodes_payload() {
        let frame = EffectStreamFrameEnvelope {
            intent_id: fake_hash('a'),
            effect_kind: "llm.session.start".into(),
            issuer_ref: Some("stream-1".into()),
            seq: 2,
            kind: "tool_call.requested".into(),
            payload: serde_cbor::to_vec(&DummyStream { chunk: 7 }).unwrap(),
            ..EffectStreamFrameEnvelope::default()
        };

        let decoded: DummyStream = frame.decode_payload().unwrap();
        assert_eq!(decoded, DummyStream { chunk: 7 });
    }

    #[test]
    fn pending_effects_bind_stream_then_settle_receipt() {
        let mut pending = PendingEffects::new();
        let handle = pending
            .begin("llm.generate", &vec!["m1"], Some("llm".into()), 11)
            .unwrap();

        let stream = EffectStreamFrameEnvelope {
            intent_id: fake_hash('i'),
            effect_kind: "llm.generate".into(),
            params_hash: Some(handle.params_hash.clone()),
            issuer_ref: Some("run-1".into()),
            seq: 1,
            kind: "progress".into(),
            payload: serde_cbor::to_vec(&DummyStream { chunk: 1 }).unwrap(),
            ..EffectStreamFrameEnvelope::default()
        };

        let matched = pending.observe((&stream).into()).expect("stream match");
        assert_eq!(
            matched.pending.intent_id.as_deref(),
            Some(stream.intent_id.as_str())
        );
        assert_eq!(matched.pending.last_stream_seq, 1);
        assert_eq!(pending.len(), 1);

        let receipt = EffectReceiptEnvelope {
            intent_id: stream.intent_id.clone(),
            effect_kind: "llm.generate".into(),
            params_hash: Some(handle.params_hash.clone()),
            issuer_ref: Some("run-1".into()),
            receipt_payload: serde_cbor::to_vec(&DummyReceipt { status: 200 }).unwrap(),
            status: "ok".into(),
            ..EffectReceiptEnvelope::default()
        };

        let settled = pending.settle((&receipt).into()).expect("settle");
        let decoded: DummyReceipt = match settled.observed {
            ObservedEffect::Settled(value) => value.decode_receipt_payload().unwrap(),
            _ => panic!("expected settled receipt"),
        };
        assert_eq!(decoded, DummyReceipt { status: 200 });
        assert!(pending.is_empty());
    }

    #[test]
    fn settle_matches_by_intent_id_when_params_hash_missing() {
        let mut pending = PendingEffects::new();
        let mut handle =
            PendingEffect::new("host.session.open", fake_hash('p'), Some("host".into()), 9);
        handle.intent_id = Some(fake_hash('i'));
        pending.insert(handle.clone());

        let rejected = EffectReceiptRejected {
            intent_id: fake_hash('i'),
            effect_kind: "host.session.open".into(),
            issuer_ref: Some("open-1".into()),
            status: "error".into(),
            error_code: "receipt.invalid_payload".into(),
            error_message: "bad payload".into(),
            payload_hash: fake_hash('x'),
            payload_size: 16,
            ..EffectReceiptRejected::default()
        };

        let matched = pending.settle((&rejected).into()).expect("rejected match");
        assert_eq!(matched.pending.params_hash, handle.params_hash);
        assert!(matches!(matched.observed, ObservedEffect::Rejected(_)));
        assert!(pending.is_empty());
    }
}
