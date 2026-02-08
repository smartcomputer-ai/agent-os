use std::collections::BTreeMap;
use std::path::Path;

use aos_cbor::Hash;
use aos_effects::{EffectKind, EffectReceipt, IntentBuilder, ReceiptStatus};
use aos_kernel::DefListing;
use aos_kernel::governance::{ManifestPatch, Proposal, ProposalState};
use aos_kernel::journal::ApprovalDecisionRecord;
use aos_kernel::patch_doc::PatchDocument;
use aos_kernel::shadow::ShadowSummary;
use aos_kernel::{KernelError, KernelHeights, ReadMeta};
use base64::prelude::*;
use jsonschema::JSONSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_cbor;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::error::HostError;
use crate::host::{ExternalEvent, WorldHost};
use crate::modes::daemon::ControlMsg;

#[derive(Clone, Copy, Debug)]
pub enum ControlMode {
    Ndjson,
    Stdio,
}

const PROTOCOL_VERSION: u8 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub v: u8,
    pub id: String,
    pub cmd: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ControlError>,
}

#[derive(Debug, Serialize, Clone)]
pub struct JournalTail {
    pub from: u64,
    pub to: u64,
    pub entries: Vec<JournalTailEntry>,
}

#[derive(Debug, Serialize, Clone)]
pub struct JournalTailEntry {
    pub kind: String,
    pub seq: u64,
    pub record: serde_json::Value,
}

impl JournalTailEntry {
    pub fn seq(&self) -> u64 {
        self.seq
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum ConsistencyJson {
    Head,
    AtLeast,
    Exact,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ControlError {
    pub code: String,
    pub message: String,
}

impl ControlError {
    fn invalid_request(msg: impl Into<String>) -> Self {
        Self {
            code: "invalid_request".into(),
            message: msg.into(),
        }
    }

    fn unknown_method() -> Self {
        Self {
            code: "unknown_method".into(),
            message: "unknown command".into(),
        }
    }

    fn decode(msg: impl Into<String>) -> Self {
        Self {
            code: "decode_error".into(),
            message: msg.into(),
        }
    }

    fn host(err: HostError) -> Self {
        Self {
            code: "host_error".into(),
            message: err.to_string(),
        }
    }
}
/// Minimal control server (Unix socket, NDJSON framing) that translates protocol
/// requests into daemon control messages and waits for responses.
pub struct ControlServer {
    path: std::path::PathBuf,
    control_tx: mpsc::Sender<ControlMsg>,
    shutdown_tx: broadcast::Sender<()>,
    shutdown_rx: broadcast::Receiver<()>,
    mode: ControlMode,
}

impl ControlServer {
    pub fn new<P: Into<std::path::PathBuf>>(
        path: P,
        control_tx: mpsc::Sender<ControlMsg>,
        shutdown_tx: broadcast::Sender<()>,
        mode: ControlMode,
    ) -> Self {
        let shutdown_rx = shutdown_tx.subscribe();
        Self {
            path: path.into(),
            control_tx,
            shutdown_tx,
            shutdown_rx,
            mode,
        }
    }

    pub async fn run(mut self) -> Result<(), HostError> {
        // Ensure no stale socket exists
        if self.path.exists() {
            let _ = std::fs::remove_file(&self.path);
        }

        let listener = UnixListener::bind(&self.path)
            .map_err(|e| HostError::External(format!("failed to bind control socket: {e}")))?;
        // Restrict permissions to owner-only (best-effort)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }

        loop {
            tokio::select! {
                res = listener.accept() => {
                    if let Ok((stream, _)) = res {
                        let tx = self.control_tx.clone();
                        let shutdown_tx = self.shutdown_tx.clone();
                        tokio::spawn(handle_conn(stream, tx, shutdown_tx, self.mode));
                    }
                }
                _ = self.shutdown_rx.recv() => {
                    let _ = std::fs::remove_file(&self.path);
                    break;
                }
            }
        }

        Ok(())
    }
}

async fn handle_conn(
    stream: UnixStream,
    control_tx: mpsc::Sender<ControlMsg>,
    shutdown_tx: broadcast::Sender<()>,
    mode: ControlMode,
) {
    match mode {
        ControlMode::Ndjson => {
            let (r, mut w) = stream.into_split();
            let mut reader = BufReader::new(r);
            let mut line = String::new();

            while let Ok(n) = reader.read_line(&mut line).await {
                if n == 0 {
                    break;
                }
                let resp = match serde_json::from_str::<RequestEnvelope>(&line) {
                    Ok(req) => handle_request(req, &control_tx, &shutdown_tx).await,
                    Err(e) => ResponseEnvelope {
                        id: "".into(),
                        ok: false,
                        result: None,
                        error: Some(ControlError::decode(e.to_string())),
                    },
                };
                if let Ok(json) = serde_json::to_string(&resp) {
                    let _ = w.write_all(json.as_bytes()).await;
                    let _ = w.write_all(b"\n").await;
                }
                line.clear();
            }
        }
        ControlMode::Stdio => {
            // Not used for Unix socket; placeholder for potential stdio support.
            let _ = shutdown_tx.send(());
        }
    }
}

pub(crate) async fn handle_request(
    req: RequestEnvelope,
    control_tx: &mpsc::Sender<ControlMsg>,
    shutdown_tx: &broadcast::Sender<()>,
) -> ResponseEnvelope {
    let id = req.id.clone();
    let res: Result<serde_json::Value, ControlError> = (|| async {
        if req.v != PROTOCOL_VERSION {
            return Err(ControlError::invalid_request(
                "unsupported protocol version",
            ));
        }
        match req.cmd.as_str() {
            "snapshot" => {
                let (tx, rx) = oneshot::channel();
                let _ = control_tx.send(ControlMsg::Snapshot { resp: tx }).await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                inner.map_err(ControlError::host)?;
                Ok(serde_json::json!({}))
            }
            "shutdown" => {
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::Shutdown {
                        resp: tx,
                        shutdown_tx: shutdown_tx.clone(),
                    })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                inner.map_err(ControlError::host)?;
                Ok(serde_json::json!({}))
            }
            "journal-head" => {
                let (tx, rx) = oneshot::channel();
                let _ = control_tx.send(ControlMsg::JournalHead { resp: tx }).await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                let meta = inner.map_err(ControlError::host)?;
                Ok(serde_json::json!({ "meta": meta_to_json(&meta) }))
            }
            "journal-list" => {
                let from = req
                    .payload
                    .get("from")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let limit = req.payload.get("limit").and_then(|v| v.as_u64());
                let kinds = req
                    .payload
                    .get("kinds")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .map(|s| s.to_string())
                            .collect::<Vec<_>>()
                    });
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::JournalTail {
                        from,
                        limit,
                        kinds,
                        resp: tx,
                    })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                let tail = inner.map_err(ControlError::host)?;
                Ok(serde_json::to_value(tail)
                    .map_err(|e| ControlError::decode(format!("encode journal: {e}")))?)
            }
            "trace-get" => {
                let event_hash = req
                    .payload
                    .get("event_hash")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let schema = req
                    .payload
                    .get("schema")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let correlate_by = req
                    .payload
                    .get("correlate_by")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let correlate_value = req
                    .payload
                    .get("value")
                    .filter(|v| !v.is_null())
                    .cloned();
                match (
                    event_hash.is_some(),
                    schema.is_some(),
                    correlate_by.is_some(),
                    correlate_value.is_some(),
                ) {
                    (true, false, false, false) => {}
                    (false, true, true, true) => {}
                    (false, false, false, false) => {
                        return Err(ControlError::invalid_request(
                            "trace-get requires either event_hash or schema+correlate_by+value",
                        ));
                    }
                    _ => {
                        return Err(ControlError::invalid_request(
                            "trace-get requires exactly one mode: event_hash or schema+correlate_by+value",
                        ));
                    }
                }
                let window_limit = req.payload.get("window_limit").and_then(|v| v.as_u64());
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::TraceGet {
                        event_hash,
                        schema,
                        correlate_by,
                        correlate_value,
                        window_limit,
                        resp: tx,
                    })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                inner.map_err(ControlError::host)
            }
            "event-send" => {
                let schema = req
                    .payload
                    .get("schema")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| ControlError::invalid_request("missing schema"))?;
                let value_b64 = req
                    .payload
                    .get("value_b64")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ControlError::invalid_request("missing value_b64"))?;
                let bytes = BASE64_STANDARD
                    .decode(value_b64)
                    .map_err(|e| ControlError::decode(format!("invalid base64: {e}")))?;
                let key = req
                    .payload
                    .get("key_b64")
                    .and_then(|v| v.as_str())
                    .map(|b64| {
                        BASE64_STANDARD
                            .decode(b64)
                            .map_err(|e| ControlError::decode(format!("invalid key base64: {e}")))
                    })
                    .transpose()?;
                let (tx, rx) = oneshot::channel();
                let evt = ExternalEvent::DomainEvent {
                    schema,
                    value: bytes,
                    key,
                };
                let _ = control_tx
                    .send(ControlMsg::EventSend {
                        event: evt,
                        resp: tx,
                    })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                inner.map_err(ControlError::host)?;
                Ok(serde_json::json!({}))
            }
            "receipt-inject" => {
                let p: InjectReceiptPayload = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let payload = BASE64_STANDARD
                    .decode(p.payload_b64)
                    .map_err(|e| ControlError::decode(format!("invalid base64: {e}")))?;
                let receipt = EffectReceipt {
                    intent_hash: p.intent_hash,
                    adapter_id: p.adapter_id,
                    status: ReceiptStatus::Ok,
                    payload_cbor: payload,
                    cost_cents: None,
                    signature: vec![],
                };
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::ReceiptInject { receipt, resp: tx })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                inner.map_err(ControlError::host)?;
                Ok(serde_json::json!({}))
            }
            "manifest-get" => {
                let consistency = req
                    .payload
                    .get("consistency")
                    .and_then(|v| v.as_str())
                    .unwrap_or("head")
                    .to_string();
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::ManifestGet {
                        consistency,
                        resp: tx,
                    })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                let (meta, manifest_bytes) = inner.map_err(ControlError::host)?;
                Ok(serde_json::json!({
                    "manifest_b64": BASE64_STANDARD.encode(manifest_bytes),
                    "meta": meta_to_json(&meta),
                }))
            }
            "def-get" | "defs-get" => {
                let name = req
                    .payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| ControlError::invalid_request("missing name"))?;
                let (tx, rx) = oneshot::channel();
                let _ = control_tx.send(ControlMsg::DefGet { name, resp: tx }).await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                let def = inner.map_err(ControlError::host)?;
                let hash = Hash::of_cbor(&def)
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                Ok(serde_json::json!({ "def": def, "hash": hash.to_hex() }))
            }
            "def-list" | "defs-list" => {
                let kinds: Option<Vec<String>> = req
                    .payload
                    .get("kinds")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    });
                let prefix = req
                    .payload
                    .get("prefix")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::DefList {
                        kinds,
                        prefix,
                        resp: tx,
                    })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                let defs: Vec<DefListing> = inner.map_err(ControlError::host)?;
                let meta = world_meta(&control_tx).await?;
                Ok(serde_json::json!({
                    "defs": defs,
                    "meta": meta_to_json(&meta),
                }))
            }
            "state-get" => {
                let reducer = req
                    .payload
                    .get("reducer")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| ControlError::invalid_request("missing reducer"))?;
                let key = req
                    .payload
                    .get("key_b64")
                    .and_then(|v| v.as_str())
                    .map(|s| BASE64_STANDARD.decode(s).unwrap_or_default());
                let consistency = req
                    .payload
                    .get("consistency")
                    .and_then(|v| v.as_str())
                    .unwrap_or("head");
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::StateGet {
                        reducer,
                        key,
                        consistency: consistency.to_string(),
                        resp: tx,
                    })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                match inner.map_err(ControlError::host)? {
                    Some((meta, bytes_opt)) => {
                        let state_b64 = bytes_opt.map(|b| BASE64_STANDARD.encode(b));
                        Ok(serde_json::json!({
                            "state_b64": state_b64,
                            "meta": meta_to_json(&meta),
                        }))
                    }
                    None => Ok(serde_json::json!({
                        "state_b64": null,
                        "meta": meta_to_json(&world_meta(&control_tx).await?),
                    })),
                }
            }
            "state-list" => {
                let reducer = req
                    .payload
                    .get("reducer")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| ControlError::invalid_request("missing reducer"))?;
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::StateList { reducer, resp: tx })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                let metas = inner.map_err(ControlError::host)?;
                let cells: Vec<serde_json::Value> = metas
                    .into_iter()
                    .map(|m| {
                        let key_b64 = BASE64_STANDARD.encode(&m.key_bytes);
                        let state_hash = Hash::from_bytes(&m.state_hash)
                            .map(|h| h.to_hex())
                            .unwrap_or_else(|_| hex::encode(m.state_hash));
                        serde_json::json!({
                            "key_b64": key_b64,
                            "state_hash_hex": state_hash,
                            "size": m.size,
                            "last_active_ns": m.last_active_ns,
                        })
                    })
                    .collect();
                let meta = world_meta(&control_tx).await?;
                Ok(serde_json::json!({ "cells": cells, "meta": meta_to_json(&meta) }))
            }

            "blob-get" => {
                let hash_hex = req
                    .payload
                    .get("hash_hex")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ControlError::invalid_request("missing hash_hex"))?
                    .to_string();
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::BlobGet { hash_hex, resp: tx })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                let bytes = inner.map_err(ControlError::host)?;
                Ok(serde_json::json!({ "data_b64": BASE64_STANDARD.encode(bytes) }))
            }
            "blob-put" => {
                let payload: PutBlobPayload = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let data = BASE64_STANDARD
                    .decode(payload.data_b64)
                    .map_err(|e| ControlError::decode(format!("invalid base64: {e}")))?;
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::PutBlob { data, resp: tx })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                let hash_hex = inner.map_err(ControlError::host)?;
                Ok(serde_json::json!({ "hash": hash_hex }))
            }
            "workspace-resolve" => {
                let params: WorkspaceResolveParams = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let receipt: WorkspaceResolveReceipt =
                    internal_effect(control_tx, EffectKind::workspace_resolve(), &params).await?;
                let value = serde_json::to_value(&receipt)
                    .map_err(|e| ControlError::decode(format!("encode receipt: {e}")))?;
                Ok(value)
            }
            "workspace-list" => {
                let params: WorkspaceListParams = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let receipt: WorkspaceListReceipt =
                    internal_effect(control_tx, EffectKind::workspace_list(), &params).await?;
                let value = serde_json::to_value(&receipt)
                    .map_err(|e| ControlError::decode(format!("encode receipt: {e}")))?;
                Ok(value)
            }
            "workspace-read-ref" => {
                let params: WorkspaceReadRefParams = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let receipt: Option<WorkspaceRefEntry> =
                    internal_effect(control_tx, EffectKind::workspace_read_ref(), &params).await?;
                let value = serde_json::to_value(&receipt)
                    .map_err(|e| ControlError::decode(format!("encode receipt: {e}")))?;
                Ok(value)
            }
            "workspace-read-bytes" => {
                let params: WorkspaceReadBytesParams = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let bytes: Vec<u8> =
                    internal_effect(control_tx, EffectKind::workspace_read_bytes(), &params)
                        .await?;
                Ok(serde_json::json!({
                    "data_b64": BASE64_STANDARD.encode(bytes)
                }))
            }
            "workspace-write-bytes" => {
                let payload: WorkspaceWriteBytesPayload =
                    serde_json::from_value(req.payload.clone())
                        .map_err(|e| ControlError::decode(format!("{e}")))?;
                let bytes = BASE64_STANDARD
                    .decode(payload.bytes_b64)
                    .map_err(|e| ControlError::decode(format!("invalid base64: {e}")))?;
                let params = WorkspaceWriteBytesParams {
                    root_hash: payload.root_hash,
                    path: payload.path,
                    bytes,
                    mode: payload.mode,
                };
                let receipt: WorkspaceWriteBytesReceipt =
                    internal_effect(control_tx, EffectKind::workspace_write_bytes(), &params)
                        .await?;
                let value = serde_json::to_value(&receipt)
                    .map_err(|e| ControlError::decode(format!("encode receipt: {e}")))?;
                Ok(value)
            }
            "workspace-remove" => {
                let params: WorkspaceRemoveParams = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let receipt: WorkspaceRemoveReceipt =
                    internal_effect(control_tx, EffectKind::workspace_remove(), &params).await?;
                let value = serde_json::to_value(&receipt)
                    .map_err(|e| ControlError::decode(format!("encode receipt: {e}")))?;
                Ok(value)
            }
            "workspace-diff" => {
                let params: WorkspaceDiffParams = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let receipt: WorkspaceDiffReceipt =
                    internal_effect(control_tx, EffectKind::workspace_diff(), &params).await?;
                let value = serde_json::to_value(&receipt)
                    .map_err(|e| ControlError::decode(format!("encode receipt: {e}")))?;
                Ok(value)
            }
            "workspace-annotations-get" => {
                let params: WorkspaceAnnotationsGetParams =
                    serde_json::from_value(req.payload.clone())
                        .map_err(|e| ControlError::decode(format!("{e}")))?;
                let receipt: WorkspaceAnnotationsGetReceipt =
                    internal_effect(control_tx, EffectKind::workspace_annotations_get(), &params)
                        .await?;
                let value = serde_json::to_value(&receipt)
                    .map_err(|e| ControlError::decode(format!("encode receipt: {e}")))?;
                Ok(value)
            }
            "workspace-annotations-set" => {
                let params: WorkspaceAnnotationsSetParams =
                    serde_json::from_value(req.payload.clone())
                        .map_err(|e| ControlError::decode(format!("{e}")))?;
                let receipt: WorkspaceAnnotationsSetReceipt =
                    internal_effect(control_tx, EffectKind::workspace_annotations_set(), &params)
                        .await?;
                let value = serde_json::to_value(&receipt)
                    .map_err(|e| ControlError::decode(format!("encode receipt: {e}")))?;
                Ok(value)
            }
            "workspace-empty-root" => {
                let params: WorkspaceEmptyRootParams = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let receipt: WorkspaceEmptyRootReceipt =
                    internal_effect(control_tx, EffectKind::workspace_empty_root(), &params)
                        .await?;
                let value = serde_json::to_value(&receipt)
                    .map_err(|e| ControlError::decode(format!("encode receipt: {e}")))?;
                Ok(value)
            }
            "gov-propose" => {
                let payload: ProposePayload = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let patch_bytes = BASE64_STANDARD
                    .decode(payload.patch_b64)
                    .map_err(|e| ControlError::decode(format!("invalid base64: {e}")))?;
                let patch = decode_governance_patch(&patch_bytes)?;
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::GovPropose {
                        patch,
                        description: payload.description,
                        resp: tx,
                    })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                let proposal_id = inner.map_err(ControlError::host)?;
                Ok(serde_json::json!({ "proposal_id": proposal_id }))
            }
            "gov-shadow" => {
                let payload: ShadowPayload = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::GovShadow {
                        proposal_id: payload.proposal_id,
                        resp: tx,
                    })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                let summary: ShadowSummary = inner.map_err(ControlError::host)?;
                let value = serde_json::to_value(&summary)
                    .map_err(|e| ControlError::decode(format!("encode summary json: {e}")))?;
                Ok(value)
            }
            "gov-approve" => {
                let payload: ApprovePayload = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let decision = match payload.decision.as_str() {
                    "approve" => ApprovalDecisionRecord::Approve,
                    "reject" => ApprovalDecisionRecord::Reject,
                    other => {
                        return Err(ControlError::decode(format!("invalid decision: {other}")));
                    }
                };
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::GovApprove {
                        proposal_id: payload.proposal_id,
                        approver: payload.approver,
                        decision,
                        resp: tx,
                    })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                inner.map_err(ControlError::host)?;
                Ok(serde_json::json!({}))
            }
            "gov-apply" => {
                let payload: ApplyPayload = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::GovApply {
                        proposal_id: payload.proposal_id,
                        resp: tx,
                    })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                inner.map_err(ControlError::host)?;
                Ok(serde_json::json!({}))
            }
            "gov-apply-direct" => {
                let payload: ApplyDirectPayload = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let patch_bytes = BASE64_STANDARD
                    .decode(payload.patch_b64)
                    .map_err(|e| ControlError::decode(format!("invalid base64: {e}")))?;
                let patch = decode_governance_patch(&patch_bytes)?;
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::GovApplyDirect { patch, resp: tx })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                let manifest_hash = inner.map_err(ControlError::host)?;
                Ok(serde_json::json!({ "manifest_hash": manifest_hash }))
            }
            "gov-list" => {
                let status = req
                    .payload
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("pending");
                let filter = parse_gov_list_filter(status)?;
                let (tx, rx) = oneshot::channel();
                let _ = control_tx.send(ControlMsg::GovList { resp: tx }).await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                let proposals = inner.map_err(ControlError::host)?;
                let list: Vec<serde_json::Value> = proposals
                    .into_iter()
                    .filter(|p| filter.matches(&p.state))
                    .map(proposal_list_json)
                    .collect();
                let meta = world_meta(&control_tx).await?;
                Ok(serde_json::json!({ "proposals": list, "meta": meta_to_json(&meta) }))
            }
            "gov-get" => {
                let proposal_id = req
                    .payload
                    .get("proposal_id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| ControlError::invalid_request("missing proposal_id"))?;
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::GovGet {
                        proposal_id,
                        resp: tx,
                    })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                let proposal = inner.map_err(ControlError::host)?;
                let meta = world_meta(&control_tx).await?;
                Ok(serde_json::json!({
                    "proposal": proposal_detail_json(proposal),
                    "meta": meta_to_json(&meta)
                }))
            }
            _ => Err(ControlError::unknown_method()),
        }
    })()
    .await;

    match res {
        Ok(val) => ResponseEnvelope {
            id,
            ok: true,
            result: Some(val),
            error: None,
        },
        Err(e) => ResponseEnvelope {
            id,
            ok: false,
            result: None,
            error: Some(e),
        },
    }
}

#[derive(Debug, Deserialize)]
struct InjectReceiptPayload {
    intent_hash: [u8; 32],
    adapter_id: String,
    payload_b64: String,
}

#[derive(Debug, Deserialize)]
struct PutBlobPayload {
    data_b64: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceResolveParams {
    workspace: String,
    #[serde(default)]
    version: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceResolveReceipt {
    exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolved_version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    head: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceEmptyRootParams {
    workspace: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceEmptyRootReceipt {
    root_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceListParams {
    root_hash: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    limit: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceListReceipt {
    entries: Vec<WorkspaceListEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceListEntry {
    path: String,
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceReadRefParams {
    root_hash: String,
    path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceRefEntry {
    kind: String,
    hash: String,
    size: u64,
    mode: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceReadBytesParams {
    root_hash: String,
    path: String,
    #[serde(default)]
    range: Option<WorkspaceReadBytesRange>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceReadBytesRange {
    start: u64,
    end: u64,
}

#[derive(Debug, Deserialize)]
struct WorkspaceWriteBytesPayload {
    root_hash: String,
    path: String,
    bytes_b64: String,
    #[serde(default)]
    mode: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceWriteBytesParams {
    root_hash: String,
    path: String,
    bytes: Vec<u8>,
    #[serde(default)]
    mode: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceWriteBytesReceipt {
    new_root_hash: String,
    blob_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceRemoveParams {
    root_hash: String,
    path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceRemoveReceipt {
    new_root_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceDiffParams {
    root_a: String,
    root_b: String,
    #[serde(default)]
    prefix: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceDiffReceipt {
    changes: Vec<WorkspaceDiffChange>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceDiffChange {
    path: String,
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    old_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    new_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(transparent)]
struct WorkspaceAnnotationsPatch(BTreeMap<String, Option<String>>);

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsGetParams {
    root_hash: String,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsGetReceipt {
    annotations: Option<WorkspaceAnnotations>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(transparent)]
struct WorkspaceAnnotations(BTreeMap<String, String>);

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsSetParams {
    root_hash: String,
    #[serde(default)]
    path: Option<String>,
    annotations_patch: WorkspaceAnnotationsPatch,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsSetReceipt {
    new_root_hash: String,
    annotations_hash: String,
}

#[derive(Debug, Deserialize)]
struct ProposePayload {
    patch_b64: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ShadowPayload {
    proposal_id: u64,
}

#[derive(Debug, Deserialize)]
struct ApprovePayload {
    proposal_id: u64,
    #[serde(default = "default_approve")]
    decision: String,
    #[serde(default = "default_approver")]
    approver: String,
}

#[derive(Debug, Deserialize)]
struct ApplyPayload {
    proposal_id: u64,
}

#[derive(Debug, Deserialize)]
struct ApplyDirectPayload {
    patch_b64: String,
}

#[derive(Debug, Clone, Copy)]
enum GovListFilter {
    Pending,
    Approved,
    Applied,
    Rejected,
    All,
    Submitted,
    Shadowed,
}

fn default_approve() -> String {
    "approve".into()
}

fn default_approver() -> String {
    "control-client".into()
}

fn parse_gov_list_filter(status: &str) -> Result<GovListFilter, ControlError> {
    match status.trim().to_ascii_lowercase().as_str() {
        "pending" => Ok(GovListFilter::Pending),
        "approved" => Ok(GovListFilter::Approved),
        "applied" => Ok(GovListFilter::Applied),
        "rejected" => Ok(GovListFilter::Rejected),
        "all" => Ok(GovListFilter::All),
        "submitted" => Ok(GovListFilter::Submitted),
        "shadowed" => Ok(GovListFilter::Shadowed),
        other => Err(ControlError::invalid_request(format!(
            "invalid status '{other}' (expected pending, approved, applied, rejected, all, submitted, shadowed)"
        ))),
    }
}

fn decode_governance_patch(
    patch_bytes: &[u8],
) -> Result<crate::modes::daemon::GovernancePatchInput, ControlError> {
    // Try ManifestPatch CBOR first; fallback to PatchDocument JSON (validated).
    if let Ok(manifest) = serde_cbor::from_slice::<ManifestPatch>(patch_bytes) {
        return Ok(crate::modes::daemon::GovernancePatchInput::Manifest(
            manifest,
        ));
    }
    if let Ok(doc_json) = serde_json::from_slice::<serde_json::Value>(patch_bytes) {
        validate_patch_doc(&doc_json)?;
        let doc: PatchDocument = serde_json::from_value(doc_json)
            .map_err(|e| ControlError::decode(format!("decode patch doc: {e}")))?;
        return Ok(crate::modes::daemon::GovernancePatchInput::PatchDoc(doc));
    }
    Err(ControlError::decode(
        "patch_b64 is neither ManifestPatch CBOR nor PatchDocument JSON",
    ))
}

impl GovListFilter {
    fn matches(self, state: &ProposalState) -> bool {
        match self {
            GovListFilter::Pending => {
                matches!(state, ProposalState::Submitted | ProposalState::Shadowed)
            }
            GovListFilter::Approved => matches!(state, ProposalState::Approved),
            GovListFilter::Applied => matches!(state, ProposalState::Applied),
            GovListFilter::Rejected => matches!(state, ProposalState::Rejected),
            GovListFilter::All => true,
            GovListFilter::Submitted => matches!(state, ProposalState::Submitted),
            GovListFilter::Shadowed => matches!(state, ProposalState::Shadowed),
        }
    }
}

fn proposal_state_str(state: &ProposalState) -> &'static str {
    match state {
        ProposalState::Submitted => "submitted",
        ProposalState::Shadowed => "shadowed",
        ProposalState::Approved => "approved",
        ProposalState::Rejected => "rejected",
        ProposalState::Applied => "applied",
    }
}

fn proposal_list_json(proposal: Proposal) -> serde_json::Value {
    let state = proposal_state_str(&proposal.state);
    serde_json::json!({
        "id": proposal.id,
        "description": proposal.description,
        "patch_hash": proposal.patch_hash,
        "state": state,
        "approver": proposal.approver,
    })
}

fn proposal_detail_json(proposal: Proposal) -> serde_json::Value {
    let state = proposal_state_str(&proposal.state);
    serde_json::json!({
        "id": proposal.id,
        "description": proposal.description,
        "patch_hash": proposal.patch_hash,
        "state": state,
        "approver": proposal.approver,
        "shadow_summary": proposal.shadow_summary,
    })
}

fn validate_patch_doc(doc: &serde_json::Value) -> Result<(), ControlError> {
    let patch_schema: serde_json::Value = serde_json::from_str(aos_air_types::schemas::PATCH)
        .map_err(|e| ControlError::decode(format!("load patch schema: {e}")))?;
    let common_schema: serde_json::Value = serde_json::from_str(aos_air_types::schemas::COMMON)
        .map_err(|e| ControlError::decode(format!("load common schema: {e}")))?;
    let mut opts = JSONSchema::options();
    opts.with_document("common.schema.json".into(), common_schema.clone());
    opts.with_document(
        "https://aos.dev/air/v1/common.schema.json".into(),
        common_schema,
    );
    let compiled = opts
        .compile(&patch_schema)
        .map_err(|e| ControlError::decode(format!("compile patch schema: {e}")))?;
    if let Err(errors) = compiled.validate(doc) {
        let msgs: Vec<String> = errors
            .map(|e| format!("{}: {}", e.instance_path, e))
            .collect();
        return Err(ControlError::decode(format!(
            "patch schema validation failed: {}",
            msgs.join("; ")
        )));
    }
    Ok(())
}

fn meta_to_json(meta: &ReadMeta) -> serde_json::Value {
    serde_json::json!({
        "journal_height": meta.journal_height,
        "snapshot_hash": meta.snapshot_hash.map(|h| h.to_hex()),
        "manifest_hash": meta.manifest_hash.to_hex(),
    })
}

async fn world_meta(control_tx: &mpsc::Sender<ControlMsg>) -> Result<ReadMeta, ControlError> {
    let (tx, rx) = oneshot::channel();
    let _ = control_tx.send(ControlMsg::JournalHead { resp: tx }).await;
    let inner = rx
        .await
        .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
    inner.map_err(ControlError::host)
}

fn decode_internal_error_message(payload: &[u8]) -> Option<String> {
    if payload.is_empty() {
        return None;
    }
    serde_cbor::from_slice::<String>(payload).ok()
}

async fn internal_effect<T: DeserializeOwned, P: Serialize>(
    control_tx: &mpsc::Sender<ControlMsg>,
    kind: EffectKind,
    params: &P,
) -> Result<T, ControlError> {
    let intent = IntentBuilder::new(kind.clone(), "sys/workspace@1", params)
        .build()
        .map_err(|e| ControlError::decode(format!("encode params: {e}")))?;
    let (tx, rx) = oneshot::channel();
    let _ = control_tx
        .send(ControlMsg::InternalEffect { intent, resp: tx })
        .await;
    let inner = rx
        .await
        .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
    let receipt = inner.map_err(ControlError::host)?;
    if receipt.status != ReceiptStatus::Ok {
        let message = decode_internal_error_message(&receipt.payload_cbor)
            .map(|msg| {
                msg.strip_prefix("query error: ")
                    .unwrap_or(msg.as_str())
                    .to_string()
            })
            .unwrap_or_else(|| "unknown error".to_string());
        return Err(ControlError::host(HostError::Kernel(KernelError::Query(
            format!("internal effect '{}' failed: {message}", kind.as_str()),
        ))));
    }
    receipt
        .payload::<T>()
        .map_err(|e| ControlError::decode(format!("decode receipt: {e}")))
}

/// Minimal control client used by tests and CLI helpers.
pub struct ControlClient {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
    timeout: std::time::Duration,
}

impl ControlClient {
    pub async fn connect(path: &Path) -> std::io::Result<Self> {
        let stream = UnixStream::connect(path).await.map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!(
                    "failed to connect to control socket {}: {e}",
                    path.display()
                ),
            )
        })?;
        let (r, w) = stream.into_split();
        let reader = BufReader::new(r);
        Ok(Self {
            reader,
            writer: w,
            timeout: std::time::Duration::from_secs(5),
        })
    }

    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub async fn send_event(
        &mut self,
        id: impl Into<String>,
        schema: &str,
        key: Option<&[u8]>,
        value_cbor: &[u8],
    ) -> std::io::Result<ResponseEnvelope> {
        let mut payload = serde_json::Map::new();
        payload.insert(
            "schema".into(),
            serde_json::Value::String(schema.to_string()),
        );
        payload.insert(
            "value_b64".into(),
            serde_json::Value::String(BASE64_STANDARD.encode(value_cbor)),
        );
        if let Some(k) = key {
            payload.insert(
                "key_b64".into(),
                serde_json::Value::String(BASE64_STANDARD.encode(k)),
            );
        }
        let env = RequestEnvelope {
            v: PROTOCOL_VERSION,
            id: id.into(),
            cmd: "event-send".into(),
            payload: serde_json::Value::Object(payload),
        };
        self.request(&env).await
    }

    pub async fn get_def(
        &mut self,
        id: impl Into<String>,
        name: &str,
    ) -> std::io::Result<ResponseEnvelope> {
        let env = RequestEnvelope {
            v: PROTOCOL_VERSION,
            id: id.into(),
            cmd: "def-get".into(),
            payload: serde_json::json!({ "name": name }),
        };
        self.request(&env).await
    }

    pub async fn list_defs(
        &mut self,
        id: impl Into<String>,
        kinds: Option<&[&str]>,
        prefix: Option<&str>,
    ) -> std::io::Result<ResponseEnvelope> {
        let kinds_val = kinds.map(|ks| {
            serde_json::Value::Array(
                ks.iter()
                    .map(|k| serde_json::Value::String(k.to_string()))
                    .collect(),
            )
        });
        let mut payload = serde_json::Map::new();
        if let Some(k) = kinds_val {
            payload.insert("kinds".into(), k);
        }
        if let Some(p) = prefix {
            payload.insert("prefix".into(), serde_json::Value::String(p.to_string()));
        }
        let env = RequestEnvelope {
            v: PROTOCOL_VERSION,
            id: id.into(),
            cmd: "defs-list".into(),
            payload: serde_json::Value::Object(payload),
        };
        self.request(&env).await
    }

    pub async fn query_state(
        &mut self,
        id: impl Into<String>,
        reducer: &str,
        key: Option<&[u8]>,
        consistency: Option<&str>,
    ) -> std::io::Result<ResponseEnvelope> {
        let mut payload = serde_json::json!({ "reducer": reducer });
        if let Some(key) = key {
            payload["key_b64"] = serde_json::json!(BASE64_STANDARD.encode(key));
        }
        if let Some(consistency) = consistency {
            payload["consistency"] = serde_json::json!(consistency);
        }

        let env = RequestEnvelope {
            v: PROTOCOL_VERSION,
            id: id.into(),
            cmd: "state-get".into(),
            payload,
        };
        self.request(&env).await
    }

    pub async fn list_cells(
        &mut self,
        id: impl Into<String>,
        reducer: &str,
    ) -> std::io::Result<ResponseEnvelope> {
        let env = RequestEnvelope {
            v: PROTOCOL_VERSION,
            id: id.into(),
            cmd: "state-list".into(),
            payload: serde_json::json!({ "reducer": reducer }),
        };
        self.request(&env).await
    }

    pub async fn shutdown(&mut self, id: impl Into<String>) -> std::io::Result<ResponseEnvelope> {
        let env = RequestEnvelope {
            v: PROTOCOL_VERSION,
            id: id.into(),
            cmd: "shutdown".into(),
            payload: serde_json::json!({}),
        };
        self.request(&env).await
    }

    pub async fn snapshot(&mut self, id: impl Into<String>) -> std::io::Result<ResponseEnvelope> {
        let env = RequestEnvelope {
            v: PROTOCOL_VERSION,
            id: id.into(),
            cmd: "snapshot".into(),
            payload: serde_json::json!({}),
        };
        self.request(&env).await
    }

    pub async fn put_blob(
        &mut self,
        id: impl Into<String>,
        data: &[u8],
    ) -> std::io::Result<ResponseEnvelope> {
        let env = RequestEnvelope {
            v: PROTOCOL_VERSION,
            id: id.into(),
            cmd: "blob-put".into(),
            payload: serde_json::json!({ "data_b64": BASE64_STANDARD.encode(data) }),
        };
        self.request(&env).await
    }

    /// Read manifest via control (returns meta + canonical CBOR bytes).
    pub async fn manifest_read(
        &mut self,
        id: impl Into<String>,
        consistency: Option<&str>,
    ) -> std::io::Result<(ReadMeta, Vec<u8>)> {
        let mut payload = serde_json::json!({});
        if let Some(c) = consistency {
            payload["consistency"] = serde_json::json!(c);
        }
        let env = RequestEnvelope {
            v: PROTOCOL_VERSION,
            id: id.into(),
            cmd: "manifest-get".into(),
            payload,
        };
        let resp = self.request(&env).await?;
        if !resp.ok {
            return Err(io_err(format!(
                "manifest-get failed: {:?}",
                resp.error.map(|e| e.message)
            )));
        }
        let result = resp
            .result
            .ok_or_else(|| io_err("manifest-get missing result"))?;
        let meta = parse_meta(
            result
                .get("meta")
                .ok_or_else(|| io_err("manifest-get missing meta"))?,
        )?;
        let manifest_b64 = result
            .get("manifest_b64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| io_err("manifest-get missing manifest_b64"))?;
        let bytes = BASE64_STANDARD
            .decode(manifest_b64)
            .map_err(|e| io_err(format!("decode manifest_b64: {e}")))?;
        Ok((meta, bytes))
    }

    /// Query reducer state with meta.
    pub async fn query_state_decoded(
        &mut self,
        id: impl Into<String>,
        reducer: &str,
        key: Option<&[u8]>,
        consistency: Option<&str>,
    ) -> std::io::Result<(ReadMeta, Option<Vec<u8>>)> {
        let resp = self.query_state(id, reducer, key, consistency).await?;
        if !resp.ok {
            return Err(io_err(format!(
                "state-get failed: {:?}",
                resp.error.map(|e| e.message)
            )));
        }
        let result = resp
            .result
            .ok_or_else(|| io_err("state-get missing result"))?;
        let meta = parse_meta(
            result
                .get("meta")
                .ok_or_else(|| io_err("state-get missing meta"))?,
        )?;
        let state_b64 = result.get("state_b64").and_then(|v| v.as_str());
        let state = match state_b64 {
            Some(s) => Some(
                BASE64_STANDARD
                    .decode(s)
                    .map_err(|e| io_err(format!("decode state_b64: {e}")))?,
            ),
            None => None,
        };
        Ok((meta, state))
    }

    /// List cells (keyed reducers) with meta.
    pub async fn list_cells_decoded(
        &mut self,
        id: impl Into<String>,
        reducer: &str,
    ) -> std::io::Result<(ReadMeta, Vec<ClientCellEntry>)> {
        let resp = self.list_cells(id, reducer).await?;
        if !resp.ok {
            return Err(io_err(format!(
                "state-list failed: {:?}",
                resp.error.map(|e| e.message)
            )));
        }
        let result = resp
            .result
            .ok_or_else(|| io_err("state-list missing result"))?;
        let meta_val = result
            .get("meta")
            .ok_or_else(|| io_err("state-list missing meta"))?;
        let meta = parse_meta(meta_val)?;
        let list = result
            .get("cells")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut cells = Vec::with_capacity(list.len());
        for item in list {
            cells.push(parse_cell_entry(item)?);
        }
        Ok((meta, cells))
    }

    /// Blob get helper.
    pub async fn blob_get(
        &mut self,
        id: impl Into<String>,
        hash_hex: &str,
    ) -> std::io::Result<Vec<u8>> {
        let env = RequestEnvelope {
            v: PROTOCOL_VERSION,
            id: id.into(),
            cmd: "blob-get".into(),
            payload: serde_json::json!({ "hash_hex": hash_hex }),
        };
        let resp = self.request(&env).await?;
        if !resp.ok {
            return Err(io_err(format!(
                "blob-get failed: {:?}",
                resp.error.map(|e| e.message)
            )));
        }
        let result = resp
            .result
            .ok_or_else(|| io_err("blob-get missing result"))?;
        let data_b64 = result
            .get("data_b64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| io_err("blob-get missing data_b64"))?;
        BASE64_STANDARD
            .decode(data_b64)
            .map_err(|e| io_err(format!("decode data_b64: {e}")))
    }

    /// Journal head meta helper.
    pub async fn journal_head_meta(&mut self, id: impl Into<String>) -> std::io::Result<ReadMeta> {
        let env = RequestEnvelope {
            v: PROTOCOL_VERSION,
            id: id.into(),
            cmd: "journal-head".into(),
            payload: serde_json::json!({}),
        };
        let resp = self.request(&env).await?;
        if !resp.ok {
            return Err(io_err(format!(
                "journal-head failed: {:?}",
                resp.error.map(|e| e.message)
            )));
        }
        let result = resp
            .result
            .ok_or_else(|| io_err("journal-head missing result"))?;
        let meta_val = result
            .get("meta")
            .ok_or_else(|| io_err("journal-head missing meta"))?;
        parse_meta(meta_val)
    }

    pub async fn request(
        &mut self,
        envelope: &RequestEnvelope,
    ) -> std::io::Result<ResponseEnvelope> {
        let json = serde_json::to_string(envelope).unwrap();
        self.writer.write_all(json.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        let mut line = String::new();
        let read = tokio::time::timeout(self.timeout, self.reader.read_line(&mut line)).await;
        let n = match read {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "control request timed out",
                ));
            }
        };
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "control connection closed",
            ));
        }
        let resp = serde_json::from_str(&line)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        Ok(resp)
    }
}

/// Helper for journal-head responses in control server.
pub fn kernel_head(host: &WorldHost<impl aos_store::Store>) -> KernelHeights {
    host.heights()
}

#[derive(Debug, Clone)]
pub struct ClientCellEntry {
    pub key_b64: String,
    pub state_hash_hex: String,
    pub size: u64,
    pub last_active_ns: u64,
}

fn parse_cell_entry(val: serde_json::Value) -> std::io::Result<ClientCellEntry> {
    let key_b64 = val
        .get("key_b64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| io_err("cell missing key_b64"))?
        .to_string();
    let state_hash_hex = val
        .get("state_hash_hex")
        .and_then(|v| v.as_str())
        .ok_or_else(|| io_err("cell missing state_hash_hex"))?
        .to_string();
    let size = val
        .get("size")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| io_err("cell missing size"))?;
    let last_active_ns = val
        .get("last_active_ns")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    Ok(ClientCellEntry {
        key_b64,
        state_hash_hex,
        size,
        last_active_ns,
    })
}

fn parse_meta(val: &serde_json::Value) -> std::io::Result<ReadMeta> {
    let journal_height = val
        .get("journal_height")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| io_err("meta missing journal_height"))?;
    let snapshot_hash = val
        .get("snapshot_hash")
        .and_then(|v| v.as_str())
        .map(|s| Hash::from_hex_str(s))
        .transpose()
        .map_err(|e| io_err(format!("snapshot_hash decode: {e}")))?;
    let manifest_hash = val
        .get("manifest_hash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| io_err("meta missing manifest_hash"))
        .and_then(|s| {
            Hash::from_hex_str(s).map_err(|e| io_err(format!("manifest_hash decode: {e}")))
        })?;
    Ok(ReadMeta {
        journal_height,
        snapshot_hash,
        manifest_hash,
    })
}

fn io_err(msg: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, msg.into())
}
