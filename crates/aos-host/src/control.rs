use std::path::Path;

use aos_cbor::Hash;
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_kernel::KernelHeights;
use aos_kernel::ReadMeta;
use aos_kernel::governance::ManifestPatch;
use aos_kernel::journal::ApprovalDecisionRecord;
use aos_kernel::patch_doc::PatchDocument;
use aos_kernel::shadow::ShadowSummary;
use base64::prelude::*;
use jsonschema::JSONSchema;
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

async fn handle_request(
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
            "send-event" => {
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
                let (tx, rx) = oneshot::channel();
                let evt = ExternalEvent::DomainEvent {
                    schema,
                    value: bytes,
                };
                let _ = control_tx
                    .send(ControlMsg::SendEvent {
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
            "inject-receipt" => {
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
                    .send(ControlMsg::InjectReceipt { receipt, resp: tx })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                inner.map_err(ControlError::host)?;
                Ok(serde_json::json!({}))
            }
            "query-state" => {
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
                    .send(ControlMsg::QueryState {
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
            "list-cells" => {
                let reducer = req
                    .payload
                    .get("reducer")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| ControlError::invalid_request("missing reducer"))?;
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::ListCells { reducer, resp: tx })
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
            "manifest-read" => {
                let consistency = req
                    .payload
                    .get("consistency")
                    .and_then(|v| v.as_str())
                    .unwrap_or("head")
                    .to_string();
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::ReadManifest {
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
            "snapshot" => {
                let (tx, rx) = oneshot::channel();
                let _ = control_tx.send(ControlMsg::Snapshot { resp: tx }).await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                inner.map_err(ControlError::host)?;
                Ok(serde_json::json!({}))
            }
            "step" => {
                let (tx, rx) = oneshot::channel();
                let _ = control_tx.send(ControlMsg::Step { resp: tx }).await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                inner.map_err(ControlError::host)?;
                Ok(serde_json::json!({ "stepped": true }))
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
            "put-blob" => {
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
            "propose" => {
                let payload: ProposePayload = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let patch_bytes = BASE64_STANDARD
                    .decode(payload.patch_b64)
                    .map_err(|e| ControlError::decode(format!("invalid base64: {e}")))?;
                // Try ManifestPatch CBOR first; fallback to PatchDocument JSON (validated).
                let patch =
                    if let Ok(manifest) = serde_cbor::from_slice::<ManifestPatch>(&patch_bytes) {
                        crate::modes::daemon::GovernancePatchInput::Manifest(manifest)
                    } else if let Ok(doc_json) =
                        serde_json::from_slice::<serde_json::Value>(&patch_bytes)
                    {
                        validate_patch_doc(&doc_json)?;
                        let doc: PatchDocument = serde_json::from_value(doc_json)
                            .map_err(|e| ControlError::decode(format!("decode patch doc: {e}")))?;
                        crate::modes::daemon::GovernancePatchInput::PatchDoc(doc)
                    } else {
                        return Err(ControlError::decode(
                            "patch_b64 is neither ManifestPatch CBOR nor PatchDocument JSON",
                        ));
                    };
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::Propose {
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
            "shadow" => {
                let payload: ShadowPayload = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::Shadow {
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
            "approve" => {
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
                    .send(ControlMsg::Approve {
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
            "apply" => {
                let payload: ApplyPayload = serde_json::from_value(req.payload.clone())
                    .map_err(|e| ControlError::decode(format!("{e}")))?;
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::Apply {
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

fn default_approve() -> String {
    "approve".into()
}

fn default_approver() -> String {
    "control-client".into()
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
        value_cbor: &[u8],
    ) -> std::io::Result<ResponseEnvelope> {
        let env = RequestEnvelope {
            v: PROTOCOL_VERSION,
            id: id.into(),
            cmd: "send-event".into(),
            payload: serde_json::json!({
                "schema": schema,
                "value_b64": BASE64_STANDARD.encode(value_cbor),
            }),
        };
        self.request(&env).await
    }

    pub async fn step(&mut self, id: impl Into<String>) -> std::io::Result<ResponseEnvelope> {
        let env = RequestEnvelope {
            v: PROTOCOL_VERSION,
            id: id.into(),
            cmd: "step".into(),
            payload: serde_json::json!({}),
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
            cmd: "query-state".into(),
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
            cmd: "list-cells".into(),
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
            cmd: "put-blob".into(),
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
            cmd: "manifest-read".into(),
            payload,
        };
        let resp = self.request(&env).await?;
        if !resp.ok {
            return Err(io_err(format!(
                "manifest-read failed: {:?}",
                resp.error.map(|e| e.message)
            )));
        }
        let result = resp
            .result
            .ok_or_else(|| io_err("manifest-read missing result"))?;
        let meta = parse_meta(
            result
                .get("meta")
                .ok_or_else(|| io_err("manifest-read missing meta"))?,
        )?;
        let manifest_b64 = result
            .get("manifest_b64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| io_err("manifest-read missing manifest_b64"))?;
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
                "query-state failed: {:?}",
                resp.error.map(|e| e.message)
            )));
        }
        let result = resp
            .result
            .ok_or_else(|| io_err("query-state missing result"))?;
        let meta = parse_meta(
            result
                .get("meta")
                .ok_or_else(|| io_err("query-state missing meta"))?,
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
                "list-cells failed: {:?}",
                resp.error.map(|e| e.message)
            )));
        }
        let result = resp
            .result
            .ok_or_else(|| io_err("list-cells missing result"))?;
        let meta_val = result
            .get("meta")
            .ok_or_else(|| io_err("list-cells missing meta"))?;
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
