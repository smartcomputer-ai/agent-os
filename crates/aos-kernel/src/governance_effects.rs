use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use aos_air_types::{HashRef, Manifest, NamedRef, Routing, SecretEntry};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_store::Store;
use serde::{Deserialize, Serialize};
use serde_cbor::Value as CborValue;

use crate::effects::EffectParamPreprocessor;
use crate::error::KernelError;
use crate::governance::ManifestPatch;
use crate::governance_utils::{self, NamedRefDiffKind, canonicalize_patch};
use crate::patch_doc::{PatchDocument, compile_patch_document};

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GovProposeParamsRaw {
    pub patch: GovPatchInput,
    #[serde(default)]
    pub summary: Option<GovPatchSummary>,
    #[serde(default)]
    pub manifest_base: Option<HashRef>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GovProposeParams {
    pub patch: GovPatchInput,
    pub summary: GovPatchSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_base: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "$tag", content = "$value")]
pub(crate) enum GovPatchInput {
    Hash(HashRef),
    #[serde(with = "serde_bytes")]
    PatchCbor(Vec<u8>),
    #[serde(with = "serde_bytes")]
    PatchDocJson(Vec<u8>),
    PatchBlobRef(GovPatchBlobRef),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GovPatchBlobRef {
    pub blob_ref: HashRef,
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GovPatchSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_manifest_hash: Option<HashRef>,
    pub patch_hash: HashRef,
    pub ops: Vec<String>,
    pub def_changes: Vec<GovDefChange>,
    pub manifest_sections: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GovDefChange {
    pub kind: String,
    pub name: String,
    pub action: GovChangeAction,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "$tag")]
pub(crate) enum GovChangeAction {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GovShadowParams {
    pub proposal_id: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GovApproveParams {
    pub proposal_id: u64,
    pub decision: GovApprovalDecision,
    pub approver: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GovApplyParams {
    pub proposal_id: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "$tag")]
pub(crate) enum GovApprovalDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GovProposeReceipt {
    pub proposal_id: u64,
    pub patch_hash: HashRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_base: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GovShadowReceipt {
    pub proposal_id: u64,
    pub manifest_hash: HashRef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub predicted_effects: Vec<GovPredictedEffect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_workflow_receipts: Vec<GovPendingWorkflowReceipt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflow_instances: Vec<GovWorkflowInstancePreview>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub module_effect_allowlists: Vec<GovModuleEffectAllowlist>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ledger_deltas: Vec<GovLedgerDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GovApproveReceipt {
    pub proposal_id: u64,
    pub decision: GovApprovalDecision,
    pub patch_hash: HashRef,
    pub approver: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GovApplyReceipt {
    pub proposal_id: u64,
    pub manifest_hash_new: HashRef,
    pub patch_hash: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GovPredictedEffect {
    pub kind: String,
    pub cap: String,
    pub intent_hash: HashRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GovPendingWorkflowReceipt {
    pub instance_id: String,
    pub origin_module_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_instance_key_b64: Option<String>,
    pub intent_hash: HashRef,
    pub effect_kind: String,
    pub emitted_at_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GovWorkflowInstancePreview {
    pub instance_id: String,
    pub status: String,
    pub last_processed_event_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_version: Option<String>,
    pub inflight_intents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GovModuleEffectAllowlist {
    pub module: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects_emitted: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GovLedgerDelta {
    pub ledger: GovLedgerKind,
    pub name: String,
    pub change: GovDeltaKind,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case", tag = "$tag")]
pub(crate) enum GovLedgerKind {
    Capability,
    Policy,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case", tag = "$tag")]
pub(crate) enum GovDeltaKind {
    Added,
    Removed,
    Changed,
}

fn decode_variant_value(value: CborValue) -> Result<(String, Option<CborValue>), String> {
    match value {
        CborValue::Map(mut map) => {
            if let Some(CborValue::Text(tag)) = map.remove(&CborValue::Text("$tag".into())) {
                let inner = map.remove(&CborValue::Text("$value".into()));
                if let Some((extra, _)) = map.into_iter().next() {
                    if let CborValue::Text(extra_key) = extra {
                        return Err(format!("unknown variant field '{extra_key}'"));
                    }
                }
                return Ok((tag, inner));
            }
            if map.len() == 1 {
                if let Some((CborValue::Text(tag), inner)) = map.into_iter().next() {
                    return Ok((tag, Some(inner)));
                }
            }
            Err("variant missing $tag".into())
        }
        _ => Err("expected variant map".into()),
    }
}

fn decode_unit_variant(tag: &str, inner: Option<CborValue>) -> Result<(), String> {
    match inner {
        None | Some(CborValue::Null) => Ok(()),
        Some(_) => Err(format!("variant '{tag}' must not carry a value")),
    }
}

fn decode_bytes_variant(tag: &str, inner: Option<CborValue>) -> Result<Vec<u8>, String> {
    let value = inner.ok_or_else(|| format!("variant '{tag}' missing value"))?;
    match value {
        CborValue::Bytes(bytes) => Ok(bytes),
        _ => Err(format!("variant '{tag}' must be bytes")),
    }
}

fn decode_text_variant(tag: &str, inner: Option<CborValue>) -> Result<String, String> {
    let value = inner.ok_or_else(|| format!("variant '{tag}' missing value"))?;
    match value {
        CborValue::Text(text) => Ok(text),
        _ => Err(format!("variant '{tag}' must be text")),
    }
}

impl<'de> Deserialize<'de> for GovPatchInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = CborValue::deserialize(deserializer)?;
        let (tag, inner) = decode_variant_value(value).map_err(serde::de::Error::custom)?;
        match tag.as_str() {
            "hash" => {
                let text = decode_text_variant(&tag, inner).map_err(serde::de::Error::custom)?;
                let hash = HashRef::new(text).map_err(serde::de::Error::custom)?;
                Ok(GovPatchInput::Hash(hash))
            }
            "patch_cbor" => {
                let bytes = decode_bytes_variant(&tag, inner).map_err(serde::de::Error::custom)?;
                Ok(GovPatchInput::PatchCbor(bytes))
            }
            "patch_doc_json" => {
                let bytes = decode_bytes_variant(&tag, inner).map_err(serde::de::Error::custom)?;
                Ok(GovPatchInput::PatchDocJson(bytes))
            }
            "patch_blob_ref" => {
                let value = inner.ok_or_else(|| {
                    serde::de::Error::custom("variant 'patch_blob_ref' missing value")
                })?;
                let blob_ref =
                    serde_cbor::value::from_value(value).map_err(serde::de::Error::custom)?;
                Ok(GovPatchInput::PatchBlobRef(blob_ref))
            }
            other => Err(serde::de::Error::custom(format!(
                "unknown GovPatchInput tag '{other}'"
            ))),
        }
    }
}

impl<'de> Deserialize<'de> for GovChangeAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = CborValue::deserialize(deserializer)?;
        let (tag, inner) = decode_variant_value(value).map_err(serde::de::Error::custom)?;
        decode_unit_variant(&tag, inner).map_err(serde::de::Error::custom)?;
        match tag.as_str() {
            "added" => Ok(GovChangeAction::Added),
            "removed" => Ok(GovChangeAction::Removed),
            "changed" => Ok(GovChangeAction::Changed),
            other => Err(serde::de::Error::custom(format!(
                "unknown GovChangeAction tag '{other}'"
            ))),
        }
    }
}

impl<'de> Deserialize<'de> for GovApprovalDecision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = CborValue::deserialize(deserializer)?;
        let (tag, inner) = decode_variant_value(value).map_err(serde::de::Error::custom)?;
        decode_unit_variant(&tag, inner).map_err(serde::de::Error::custom)?;
        match tag.as_str() {
            "approve" => Ok(GovApprovalDecision::Approve),
            "reject" => Ok(GovApprovalDecision::Reject),
            other => Err(serde::de::Error::custom(format!(
                "unknown GovApprovalDecision tag '{other}'"
            ))),
        }
    }
}

impl<'de> Deserialize<'de> for GovLedgerKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = CborValue::deserialize(deserializer)?;
        let (tag, inner) = decode_variant_value(value).map_err(serde::de::Error::custom)?;
        decode_unit_variant(&tag, inner).map_err(serde::de::Error::custom)?;
        match tag.as_str() {
            "capability" => Ok(GovLedgerKind::Capability),
            "policy" => Ok(GovLedgerKind::Policy),
            other => Err(serde::de::Error::custom(format!(
                "unknown GovLedgerKind tag '{other}'"
            ))),
        }
    }
}

impl<'de> Deserialize<'de> for GovDeltaKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = CborValue::deserialize(deserializer)?;
        let (tag, inner) = decode_variant_value(value).map_err(serde::de::Error::custom)?;
        decode_unit_variant(&tag, inner).map_err(serde::de::Error::custom)?;
        match tag.as_str() {
            "added" => Ok(GovDeltaKind::Added),
            "removed" => Ok(GovDeltaKind::Removed),
            "changed" => Ok(GovDeltaKind::Changed),
            other => Err(serde::de::Error::custom(format!(
                "unknown GovDeltaKind tag '{other}'"
            ))),
        }
    }
}

pub struct GovernanceParamPreprocessor<S: Store> {
    store: Arc<S>,
    manifest: Arc<Manifest>,
}

impl<S: Store> GovernanceParamPreprocessor<S> {
    pub fn new(store: Arc<S>, manifest: Manifest) -> Self {
        Self {
            store,
            manifest: Arc::new(manifest),
        }
    }
}

impl<S: Store> EffectParamPreprocessor for GovernanceParamPreprocessor<S> {
    fn preprocess(
        &self,
        _source: &aos_effects::EffectSource,
        kind: &aos_effects::EffectKind,
        params_cbor: Vec<u8>,
    ) -> Result<Vec<u8>, KernelError> {
        if kind.as_str() != "governance.propose" {
            return Ok(params_cbor);
        }
        let raw: GovProposeParamsRaw = serde_cbor::from_slice(&params_cbor)
            .map_err(|err| KernelError::Manifest(format!("decode gov.propose params: {err}")))?;
        let prepared = prepare_patch(
            self.store.as_ref(),
            self.manifest.as_ref(),
            raw.patch,
            raw.manifest_base.clone(),
        )?;
        let mut manifest_base = raw.manifest_base;
        if manifest_base.is_some() && prepared.base_manifest_hash.is_none() {
            return Err(KernelError::Manifest(
                "manifest_base supplied but patch input does not include a base manifest hash"
                    .into(),
            ));
        }
        if let (Some(expected), Some(actual)) =
            (manifest_base.as_ref(), prepared.base_manifest_hash.as_ref())
        {
            if expected != actual {
                return Err(KernelError::Manifest(format!(
                    "manifest_base mismatch: expected {expected}, got {actual}"
                )));
            }
        }
        if manifest_base.is_none() {
            manifest_base = prepared.base_manifest_hash.clone();
        }
        let params = GovProposeParams {
            patch: GovPatchInput::Hash(prepared.patch_hash.clone()),
            summary: prepared.summary,
            manifest_base,
            description: raw.description,
        };
        to_canonical_cbor(&params).map_err(|err| KernelError::Manifest(err.to_string()))
    }
}

pub(crate) struct PreparedPatch {
    pub patch: ManifestPatch,
    pub patch_hash: HashRef,
    pub base_manifest_hash: Option<HashRef>,
    pub summary: GovPatchSummary,
}

pub(crate) fn prepare_patch<S: Store>(
    store: &S,
    current_manifest: &Manifest,
    input: GovPatchInput,
    manifest_base: Option<HashRef>,
) -> Result<PreparedPatch, KernelError> {
    match input {
        GovPatchInput::Hash(hash) => {
            if manifest_base.is_some() {
                return Err(KernelError::Manifest(
                    "manifest_base is not supported with patch hash input".into(),
                ));
            }
            let patch = load_patch_by_hash(store, &hash)?;
            let summary = build_patch_summary(current_manifest, &patch, None, &hash)?;
            Ok(PreparedPatch {
                patch,
                patch_hash: hash,
                base_manifest_hash: None,
                summary,
            })
        }
        GovPatchInput::PatchCbor(bytes) => {
            if manifest_base.is_some() {
                return Err(KernelError::Manifest(
                    "manifest_base is not supported with patch_cbor input".into(),
                ));
            }
            let patch = decode_patch_cbor(&bytes)?;
            let patch = canonicalize_patch(store, patch)?;
            let (patch_hash, patch_bytes) = store_patch(store, &patch)?;
            let summary = build_patch_summary(current_manifest, &patch, None, &patch_hash)?;
            Ok(PreparedPatch {
                patch,
                patch_hash,
                base_manifest_hash: None,
                summary,
            })
        }
        GovPatchInput::PatchDocJson(bytes) => {
            let (patch, base_hash) = compile_patch_doc(store, &bytes)?;
            if let Some(expected) = manifest_base.as_ref() {
                if expected != &base_hash {
                    return Err(KernelError::Manifest(format!(
                        "manifest_base mismatch: expected {expected}, got {base_hash}"
                    )));
                }
            }
            let (patch_hash, _patch_bytes) = store_patch(store, &patch)?;
            let base_manifest = load_manifest_by_hash(store, &base_hash)?;
            let summary =
                build_patch_summary(&base_manifest, &patch, Some(&base_hash), &patch_hash)?;
            Ok(PreparedPatch {
                patch,
                patch_hash,
                base_manifest_hash: Some(base_hash),
                summary,
            })
        }
        GovPatchInput::PatchBlobRef(blob_ref) => {
            let hash = Hash::from_hex_str(blob_ref.blob_ref.as_str())
                .map_err(|err| KernelError::Manifest(format!("invalid blob_ref: {err}")))?;
            let bytes = store
                .get_blob(hash)
                .map_err(|err| KernelError::Manifest(format!("load patch blob: {err}")))?;
            match blob_ref.format.as_str() {
                "manifest_patch_cbor" => prepare_patch(
                    store,
                    current_manifest,
                    GovPatchInput::PatchCbor(bytes),
                    manifest_base,
                ),
                "patch_doc_json" => prepare_patch(
                    store,
                    current_manifest,
                    GovPatchInput::PatchDocJson(bytes),
                    manifest_base,
                ),
                other => Err(KernelError::Manifest(format!(
                    "unknown patch blob format '{other}'"
                ))),
            }
        }
    }
}

pub(crate) fn decode_patch_cbor(bytes: &[u8]) -> Result<ManifestPatch, KernelError> {
    serde_cbor::from_slice::<ManifestPatch>(bytes)
        .map_err(|err| KernelError::Manifest(format!("decode patch cbor: {err}")))
}

fn compile_patch_doc<S: Store>(
    store: &S,
    bytes: &[u8],
) -> Result<(ManifestPatch, HashRef), KernelError> {
    let doc_json: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|err| KernelError::Manifest(format!("decode patch doc json: {err}")))?;
    let doc: PatchDocument = serde_json::from_value(doc_json)
        .map_err(|err| KernelError::Manifest(format!("decode patch doc: {err}")))?;
    let base_hash = HashRef::new(doc.base_manifest_hash.clone())
        .map_err(|err| KernelError::Manifest(format!("invalid base_manifest_hash: {err}")))?;
    let patch = compile_patch_document(store, doc)?;
    Ok((patch, base_hash))
}

fn store_patch<S: Store>(
    store: &S,
    patch: &ManifestPatch,
) -> Result<(HashRef, Vec<u8>), KernelError> {
    let patch_bytes = to_canonical_cbor(patch)
        .map_err(|err| KernelError::Manifest(format!("encode patch: {err}")))?;
    let hash = store
        .put_blob(&patch_bytes)
        .map_err(|err| KernelError::Manifest(format!("store patch: {err}")))?;
    let hash_ref =
        HashRef::new(hash.to_hex()).map_err(|err| KernelError::Manifest(err.to_string()))?;
    Ok((hash_ref, patch_bytes))
}

pub(crate) fn load_patch_by_hash<S: Store>(
    store: &S,
    hash: &HashRef,
) -> Result<ManifestPatch, KernelError> {
    let hash = Hash::from_hex_str(hash.as_str())
        .map_err(|err| KernelError::Manifest(format!("invalid patch hash: {err}")))?;
    let bytes = store
        .get_blob(hash)
        .map_err(|err| KernelError::Manifest(format!("load patch: {err}")))?;
    decode_patch_cbor(&bytes)
}

fn load_manifest_by_hash<S: Store>(store: &S, hash: &HashRef) -> Result<Manifest, KernelError> {
    let hash = Hash::from_hex_str(hash.as_str())
        .map_err(|err| KernelError::Manifest(format!("invalid manifest hash: {err}")))?;
    let node: aos_air_types::AirNode = store
        .get_node(hash)
        .map_err(|err| KernelError::Manifest(format!("load manifest: {err}")))?;
    match node {
        aos_air_types::AirNode::Manifest(manifest) => Ok(manifest),
        _ => Err(KernelError::Manifest(
            "base_manifest_hash did not point to a manifest node".into(),
        )),
    }
}

fn build_patch_summary(
    base_manifest: &Manifest,
    patch: &ManifestPatch,
    base_hash: Option<&HashRef>,
    patch_hash: &HashRef,
) -> Result<GovPatchSummary, KernelError> {
    let mut def_changes = Vec::new();
    let mut refs_changed = false;

    refs_changed |= push_named_ref_changes(
        "defschema",
        &base_manifest.schemas,
        &patch.manifest.schemas,
        &mut def_changes,
    );
    refs_changed |= push_named_ref_changes(
        "defmodule",
        &base_manifest.modules,
        &patch.manifest.modules,
        &mut def_changes,
    );
    refs_changed |= push_named_ref_changes(
        "defeffect",
        &base_manifest.effects,
        &patch.manifest.effects,
        &mut def_changes,
    );
    refs_changed |= push_named_ref_changes(
        "defcap",
        &base_manifest.caps,
        &patch.manifest.caps,
        &mut def_changes,
    );
    refs_changed |= push_named_ref_changes(
        "defpolicy",
        &base_manifest.policies,
        &patch.manifest.policies,
        &mut def_changes,
    );
    refs_changed |= diff_secret_refs(
        &base_manifest.secrets,
        &patch.manifest.secrets,
        &mut def_changes,
    )?;

    def_changes.sort_by(|a, b| {
        let a_key = (&a.kind, &a.name, change_rank(a.action));
        let b_key = (&b.kind, &b.name, change_rank(b.action));
        a_key.cmp(&b_key)
    });

    let mut manifest_sections = HashSet::new();
    if section_changed(&base_manifest.defaults, &patch.manifest.defaults)? {
        manifest_sections.insert("defaults".to_string());
    }
    let base_routing = base_manifest.routing.clone().unwrap_or_else(|| Routing {
        subscriptions: Vec::new(),
        inboxes: Vec::new(),
    });
    let next_routing = patch.manifest.routing.clone().unwrap_or_else(|| Routing {
        subscriptions: Vec::new(),
        inboxes: Vec::new(),
    });
    if section_changed(&base_routing.subscriptions, &next_routing.subscriptions)? {
        manifest_sections.insert("routing_events".to_string());
    }
    if section_changed(&base_routing.inboxes, &next_routing.inboxes)? {
        manifest_sections.insert("routing_inboxes".to_string());
    }
    if section_changed(
        &base_manifest.module_bindings,
        &patch.manifest.module_bindings,
    )? {
        manifest_sections.insert("module_bindings".to_string());
    }
    if section_changed(&base_manifest.secrets, &patch.manifest.secrets)? {
        manifest_sections.insert("secrets".to_string());
    }
    if refs_changed {
        manifest_sections.insert("manifest_refs".to_string());
    }

    let mut ops = HashSet::new();
    for change in &def_changes {
        match change.action {
            GovChangeAction::Added => {
                ops.insert("add_def".to_string());
            }
            GovChangeAction::Removed => {
                ops.insert("remove_def".to_string());
            }
            GovChangeAction::Changed => {
                ops.insert("replace_def".to_string());
            }
        }
    }
    if refs_changed {
        ops.insert("set_manifest_refs".to_string());
    }
    for section in &manifest_sections {
        match section.as_str() {
            "defaults" => {
                ops.insert("set_defaults".to_string());
            }
            "routing_events" => {
                ops.insert("set_routing_events".to_string());
            }
            "routing_inboxes" => {
                ops.insert("set_routing_inboxes".to_string());
            }
            "module_bindings" => {
                ops.insert("set_module_bindings".to_string());
            }
            "secrets" => {
                ops.insert("set_secrets".to_string());
            }
            "manifest_refs" => {}
            _ => {}
        }
    }

    let mut ops = ops.into_iter().collect::<Vec<_>>();
    ops.sort();
    let mut manifest_sections = manifest_sections.into_iter().collect::<Vec<_>>();
    manifest_sections.sort();

    Ok(GovPatchSummary {
        base_manifest_hash: base_hash.cloned(),
        patch_hash: patch_hash.clone(),
        ops,
        def_changes,
        manifest_sections,
    })
}

fn push_named_ref_changes(
    kind: &str,
    base: &[NamedRef],
    next: &[NamedRef],
    changes: &mut Vec<GovDefChange>,
) -> bool {
    let mut changed = false;
    for delta in governance_utils::diff_named_refs(base, next) {
        changes.push(GovDefChange {
            kind: kind.to_string(),
            name: delta.name,
            action: match delta.change {
                NamedRefDiffKind::Added => GovChangeAction::Added,
                NamedRefDiffKind::Removed => GovChangeAction::Removed,
                NamedRefDiffKind::Changed => GovChangeAction::Changed,
            },
        });
        changed = true;
    }
    changed
}

fn diff_secret_refs(
    base: &[SecretEntry],
    next: &[SecretEntry],
    changes: &mut Vec<GovDefChange>,
) -> Result<bool, KernelError> {
    let base_map = map_secrets(base)?;
    let next_map = map_secrets(next)?;
    let mut changed = false;
    for (name, hash) in &next_map {
        match base_map.get(name) {
            None => {
                changes.push(GovDefChange {
                    kind: "defsecret".to_string(),
                    name: name.clone(),
                    action: GovChangeAction::Added,
                });
                changed = true;
            }
            Some(existing) if existing != hash => {
                changes.push(GovDefChange {
                    kind: "defsecret".to_string(),
                    name: name.clone(),
                    action: GovChangeAction::Changed,
                });
                changed = true;
            }
            _ => {}
        }
    }
    for name in base_map.keys() {
        if !next_map.contains_key(name) {
            changes.push(GovDefChange {
                kind: "defsecret".to_string(),
                name: name.clone(),
                action: GovChangeAction::Removed,
            });
            changed = true;
        }
    }
    Ok(changed)
}

fn map_secrets(secrets: &[SecretEntry]) -> Result<HashMap<String, String>, KernelError> {
    let mut map = HashMap::new();
    for entry in secrets {
        let (name, hash) = secret_entry_identity(entry)?;
        map.insert(name, hash);
    }
    Ok(map)
}

fn secret_entry_identity(entry: &SecretEntry) -> Result<(String, String), KernelError> {
    match entry {
        SecretEntry::Ref(reference) => Ok((
            reference.name.as_str().to_string(),
            reference.hash.as_str().to_string(),
        )),
        SecretEntry::Decl(decl) => {
            let name = format!("{}@{}", decl.alias, decl.version);
            let hash = Hash::of_cbor(entry)
                .map_err(|err| KernelError::Manifest(format!("hash secret decl: {err}")))?;
            Ok((name, hash.to_hex()))
        }
    }
}

fn section_changed<T: Serialize>(base: &T, next: &T) -> Result<bool, KernelError> {
    let base_bytes = to_canonical_cbor(base)
        .map_err(|err| KernelError::Manifest(format!("encode manifest section: {err}")))?;
    let next_bytes = to_canonical_cbor(next)
        .map_err(|err| KernelError::Manifest(format!("encode manifest section: {err}")))?;
    Ok(base_bytes != next_bytes)
}

fn change_rank(action: GovChangeAction) -> u8 {
    match action {
        GovChangeAction::Added => 0,
        GovChangeAction::Changed => 1,
        GovChangeAction::Removed => 2,
    }
}
