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
