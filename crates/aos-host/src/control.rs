use std::path::Path;

use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_kernel::KernelHeights;
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
                let (tx, rx) = oneshot::channel();
                let _ = control_tx
                    .send(ControlMsg::QueryState {
                        reducer,
                        key,
                        resp: tx,
                    })
                    .await;
                let inner = rx
                    .await
                    .map_err(|e| ControlError::host(HostError::External(e.to_string())))?;
                match inner.map_err(ControlError::host)? {
                    Some(bytes) => Ok(serde_json::json!({
                        "state_b64": BASE64_STANDARD.encode(bytes)
                    })),
                    None => Ok(serde_json::json!({ "state_b64": null })),
                }
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
                let head = inner.map_err(ControlError::host)?;
                Ok(serde_json::json!({ "head": head }))
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
    ) -> std::io::Result<ResponseEnvelope> {
        let env = RequestEnvelope {
            v: PROTOCOL_VERSION,
            id: id.into(),
            cmd: "query-state".into(),
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
