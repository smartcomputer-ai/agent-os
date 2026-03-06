use alloc::collections::BTreeMap;
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
            cap_slot,
            emitted_at_ns,
            last_stream_seq: 0,
        }
    }

    pub fn from_params<T: Serialize>(
        effect_kind: impl Into<String>,
        params: &T,
        cap_slot: Option<String>,
        emitted_at_ns: u64,
    ) -> Result<Self, serde_cbor::Error> {
        Ok(Self::new(
            effect_kind,
            effect_params_hash(params)?,
            cap_slot,
            emitted_at_ns,
        ))
    }

    pub fn matches(&self, continuation: EffectContinuationRef<'_>) -> bool {
        if self.effect_kind != continuation.effect_kind() {
            return false;
        }

        if let Some(intent_id) = self.intent_id.as_deref() {
            return intent_id == continuation.intent_id();
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
        let pending = PendingEffect::from_params(effect_kind, params, cap_slot, emitted_at_ns)?;
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
            return Some(params_hash.clone());
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HashRef {
    pub algorithm: String,
    #[serde(with = "serde_bytes_vec")]
    pub digest: Vec<u8>,
}

pub type HeaderMap = BTreeMap<String, String>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestTimings {
    pub start_ns: u64,
    pub end_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpRequestParams {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: HeaderMap,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_ref: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpRequestReceipt {
    pub status: i32,
    #[serde(default)]
    pub headers: HeaderMap,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_ref: Option<HashRef>,
    pub timings: RequestTimings,
    pub adapter_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobPutParams {
    #[serde(with = "serde_bytes_vec")]
    pub bytes: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs: Option<Vec<HashRef>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobEdge {
    pub blob_ref: HashRef,
    pub refs: Vec<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobPutReceipt {
    pub blob_ref: HashRef,
    pub edge_ref: HashRef,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobGetParams {
    pub blob_ref: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobGetReceipt {
    pub blob_ref: HashRef,
    pub size: u64,
    #[serde(with = "serde_bytes_vec")]
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimerSetParams {
    pub deliver_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimerSetReceipt {
    pub delivered_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostMount {
    pub host_path: String,
    pub guest_path: String,
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostLocalTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mounts: Option<Vec<HostMount>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    pub network_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value", rename_all = "snake_case")]
pub enum HostTarget {
    Local(HostLocalTarget),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSessionOpenParams {
    pub target: HostTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_ttl_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSessionOpenReceipt {
    pub session_id: String,
    pub status: String,
    pub started_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostInlineText {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostInlineBytes {
    #[serde(with = "serde_bytes_vec")]
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostBlobOutput {
    pub blob_ref: HashRef,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "serde_bytes_opt")]
    pub preview_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum HostOutput {
    InlineText { inline_text: HostInlineText },
    InlineBytes { inline_bytes: HostInlineBytes },
    Blob { blob: HostBlobOutput },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostExecParams {
    pub session_id: String,
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_patch: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostExecReceipt {
    pub exit_code: i32,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<HostOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<HostOutput>,
    pub started_at_ns: u64,
    pub ended_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSessionSignalParams {
    pub session_id: String,
    pub signal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace_timeout_ns: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSessionSignalReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostBlobRefInput {
    pub blob_ref: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum HostFileContentInput {
    InlineText { inline_text: HostInlineText },
    InlineBytes { inline_bytes: HostInlineBytes },
    BlobRef { blob_ref: HostBlobRefInput },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsReadFileParams {
    pub session_id: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsReadFileReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<HostOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsWriteFileParams {
    pub session_id: String,
    pub path: String,
    pub content: HostFileContentInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_parents: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsWriteFileReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_mtime_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsEditFileParams {
    pub session_id: String,
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_all: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsEditFileReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacements: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum HostPatchInput {
    InlineText { inline_text: HostInlineText },
    BlobRef { blob_ref: HostBlobRefInput },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostPatchOpsSummary {
    pub add: u64,
    pub update: u64,
    pub delete: u64,
    pub r#move: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsApplyPatchParams {
    pub session_id: String,
    pub patch: HostPatchInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsApplyPatchReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_changed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_paths: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops: Option<HostPatchOpsSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum HostTextOutput {
    InlineText { inline_text: HostInlineText },
    Blob { blob: HostBlobOutput },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsGrepParams {
    pub session_id: String,
    pub pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob_filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_insensitive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsGrepReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<HostTextOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsGlobParams {
    pub session_id: String,
    pub pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsGlobReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<HostTextOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsStatParams {
    pub session_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsStatReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_dir: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsExistsParams {
    pub session_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsExistsReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsListDirParams {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsListDirReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<HostTextOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretRef {
    pub alias: String,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextOrSecretRef {
    Literal(String),
    Secret(SecretRef),
}

impl TextOrSecretRef {
    pub fn literal(value: impl Into<String>) -> Self {
        Self::Literal(value.into())
    }

    pub fn secret(alias: impl Into<String>, version: u64) -> Self {
        Self::Secret(SecretRef {
            alias: alias.into(),
            version,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "$tag", content = "$value")]
enum TaggedTextOrSecretRef<'a> {
    #[serde(rename = "literal")]
    Literal(&'a str),
    #[serde(rename = "secret")]
    Secret(&'a SecretRef),
}

impl Serialize for TextOrSecretRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Literal(value) => {
                TaggedTextOrSecretRef::Literal(value.as_str()).serialize(serializer)
            }
            Self::Secret(value) => TaggedTextOrSecretRef::Secret(value).serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for TextOrSecretRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_cbor::Value::deserialize(deserializer)?;
        text_or_secret_from_value(value).map_err(serde::de::Error::custom)
    }
}

fn text_or_secret_from_value(value: serde_cbor::Value) -> Result<TextOrSecretRef, String> {
    match value {
        serde_cbor::Value::Text(text) => Ok(TextOrSecretRef::Literal(text)),
        serde_cbor::Value::Bytes(bytes) => {
            let text = String::from_utf8(bytes)
                .map_err(|err| alloc::format!("literal bytes not utf8: {err}"))?;
            Ok(TextOrSecretRef::Literal(text))
        }
        serde_cbor::Value::Map(map) => {
            if let Some(serde_cbor::Value::Text(tag)) =
                map.get(&serde_cbor::Value::Text("$tag".into()))
            {
                let payload = map
                    .get(&serde_cbor::Value::Text("$value".into()))
                    .ok_or_else(|| "missing '$value' in tagged variant".to_string())?;
                return parse_tagged_text_or_secret(tag, payload);
            }
            Err("expected text/bytes or tagged variant for TextOrSecretRef".to_string())
        }
        other => Err(alloc::format!(
            "unsupported value for TextOrSecretRef: {other:?}"
        )),
    }
}

fn parse_tagged_text_or_secret(
    tag: &str,
    payload: &serde_cbor::Value,
) -> Result<TextOrSecretRef, String> {
    match tag {
        "literal" => match payload {
            serde_cbor::Value::Text(text) => Ok(TextOrSecretRef::Literal(text.clone())),
            serde_cbor::Value::Bytes(bytes) => {
                let text = String::from_utf8(bytes.clone())
                    .map_err(|err| alloc::format!("literal bytes not utf8: {err}"))?;
                Ok(TextOrSecretRef::Literal(text))
            }
            other => Err(alloc::format!(
                "literal payload must be text or bytes, got {other:?}"
            )),
        },
        "secret" => {
            let serde_cbor::Value::Map(map) = payload else {
                return Err("secret payload must be a map".to_string());
            };
            let alias = match map.get(&serde_cbor::Value::Text("alias".into())) {
                Some(serde_cbor::Value::Text(alias)) => alias.clone(),
                Some(other) => {
                    return Err(alloc::format!("secret alias must be text, got {other:?}"));
                }
                None => return Err("secret payload missing alias".to_string()),
            };
            let version = match map.get(&serde_cbor::Value::Text("version".into())) {
                Some(serde_cbor::Value::Integer(version)) if *version >= 0 => *version as u64,
                Some(other) => {
                    return Err(alloc::format!("secret version must be nat, got {other:?}"));
                }
                None => return Err("secret payload missing version".to_string()),
            };
            Ok(TextOrSecretRef::Secret(SecretRef { alias, version }))
        }
        other => Err(alloc::format!("unsupported TextOrSecretRef tag '{other}'")),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum LlmToolChoice {
    Auto,
    #[serde(rename = "None")]
    NoneChoice,
    Required,
    Tool {
        name: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmRuntimeArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_refs: Option<Vec<HashRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<LlmToolChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format_ref: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmGenerateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub message_refs: Vec<HashRef>,
    pub runtime: LlmRuntimeArgs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<TextOrSecretRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmFinishReason {
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub arguments_ref: HashRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_call_id: Option<String>,
}

pub type LlmToolCallList = Vec<LlmToolCall>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmOutputEnvelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_ref: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmUsageDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt: u64,
    pub completion: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmGenerateReceipt {
    pub output_ref: HashRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_output_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_response_id: Option<String>,
    pub finish_reason: LlmFinishReason,
    pub token_usage: TokenUsage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_details: Option<LlmUsageDetails>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warnings_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cents: Option<u64>,
    pub provider_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultPutParams {
    pub alias: String,
    pub binding_id: String,
    pub value_ref: HashRef,
    pub expected_digest: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultPutReceipt {
    pub alias: String,
    pub version: u64,
    pub binding_id: String,
    pub digest: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultRotateParams {
    pub alias: String,
    pub version: u64,
    pub binding_id: String,
    pub expected_digest: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultRotateReceipt {
    pub alias: String,
    pub version: u64,
    pub binding_id: String,
    pub digest: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum GovPatchInput {
    Hash(HashRef),
    PatchCbor(#[serde(with = "serde_bytes_vec")] Vec<u8>),
    PatchDocJson(#[serde(with = "serde_bytes_vec")] Vec<u8>),
    PatchBlobRef { blob_ref: HashRef, format: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum GovChangeAction {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovDefChange {
    pub kind: String,
    pub name: String,
    pub action: GovChangeAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovPatchSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_manifest_hash: Option<HashRef>,
    pub patch_hash: HashRef,
    pub ops: Vec<String>,
    pub def_changes: Vec<GovDefChange>,
    pub manifest_sections: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovProposeParams {
    pub patch: GovPatchInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<GovPatchSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_base: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovProposeReceipt {
    pub proposal_id: u64,
    pub patch_hash: HashRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_base: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovShadowParams {
    pub proposal_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovPredictedEffect {
    pub kind: String,
    pub cap: String,
    pub intent_hash: HashRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovPendingWorkflowReceipt {
    pub instance_id: String,
    pub origin_module_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_instance_key_b64: Option<String>,
    pub intent_hash: HashRef,
    pub effect_kind: String,
    pub emitted_at_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovWorkflowInstancePreview {
    pub instance_id: String,
    pub status: String,
    pub last_processed_event_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_version: Option<String>,
    pub inflight_intents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovModuleEffectAllowlist {
    pub module: String,
    pub effects_emitted: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum GovLedgerKind {
    Capability,
    Policy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum GovLedgerChange {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovLedgerDelta {
    pub ledger: GovLedgerKind,
    pub name: String,
    pub change: GovLedgerChange,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovShadowReceipt {
    pub proposal_id: u64,
    pub manifest_hash: HashRef,
    pub predicted_effects: Vec<GovPredictedEffect>,
    pub pending_workflow_receipts: Vec<GovPendingWorkflowReceipt>,
    pub workflow_instances: Vec<GovWorkflowInstancePreview>,
    pub module_effect_allowlists: Vec<GovModuleEffectAllowlist>,
    pub ledger_deltas: Vec<GovLedgerDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum GovDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovApproveParams {
    pub proposal_id: u64,
    pub decision: GovDecision,
    pub approver: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovApproveReceipt {
    pub proposal_id: u64,
    pub decision: GovDecision,
    pub patch_hash: HashRef,
    pub approver: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovApplyParams {
    pub proposal_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovApplyReceipt {
    pub proposal_id: u64,
    pub manifest_hash_new: HashRef,
    pub patch_hash: HashRef,
}

pub type WorkspaceAnnotations = BTreeMap<String, HashRef>;
pub type WorkspaceAnnotationsPatch = BTreeMap<String, Option<HashRef>>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceResolveParams {
    pub workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceResolveReceipt {
    pub exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_hash: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceListParams {
    pub root_hash: HashRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    pub limit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceListEntry {
    pub path: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceListReceipt {
    pub entries: Vec<WorkspaceListEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceReadRefParams {
    pub root_hash: HashRef,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRefEntry {
    pub kind: String,
    pub hash: HashRef,
    pub size: u64,
    pub mode: u64,
}

pub type WorkspaceReadRefReceipt = Option<WorkspaceRefEntry>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceReadBytesRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceReadBytesParams {
    pub root_hash: HashRef,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<WorkspaceReadBytesRange>,
}

pub type WorkspaceReadBytesReceipt = Vec<u8>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceWriteBytesParams {
    pub root_hash: HashRef,
    pub path: String,
    #[serde(with = "serde_bytes_vec")]
    pub bytes: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceWriteBytesReceipt {
    pub new_root_hash: HashRef,
    pub blob_hash: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRemoveParams {
    pub root_hash: HashRef,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRemoveReceipt {
    pub new_root_hash: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceDiffParams {
    pub root_a: HashRef,
    pub root_b: HashRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceDiffChange {
    pub path: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_hash: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_hash: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceDiffReceipt {
    pub changes: Vec<WorkspaceDiffChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceAnnotationsGetParams {
    pub root_hash: HashRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceAnnotationsGetReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<WorkspaceAnnotations>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceAnnotationsSetParams {
    pub root_hash: HashRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub annotations_patch: WorkspaceAnnotationsPatch,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceAnnotationsSetReceipt {
    pub new_root_hash: HashRef,
    pub annotations_hash: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceEmptyRootParams {
    pub workspace: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceEmptyRootReceipt {
    pub root_hash: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadMeta {
    pub journal_height: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_hash: Option<HashRef>,
    pub manifest_hash: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrospectManifestParams {
    pub consistency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrospectManifestReceipt {
    #[serde(with = "serde_bytes_vec")]
    pub manifest: Vec<u8>,
    pub meta: ReadMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrospectWorkflowStateParams {
    pub workflow: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "serde_bytes_opt")]
    pub key: Option<Vec<u8>>,
    pub consistency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrospectWorkflowStateReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "serde_bytes_opt")]
    pub state: Option<Vec<u8>>,
    pub meta: ReadMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct IntrospectJournalHeadParams {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrospectJournalHeadReceipt {
    pub meta: ReadMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrospectListCellsParams {
    pub workflow: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrospectCellInfo {
    #[serde(with = "serde_bytes_vec")]
    pub key: Vec<u8>,
    pub state_hash: HashRef,
    pub size: u64,
    pub last_active_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrospectListCellsReceipt {
    pub cells: Vec<IntrospectCellInfo>,
    pub meta: ReadMeta,
}

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
