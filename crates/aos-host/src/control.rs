use std::path::Path;

use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_kernel::KernelHeights;
use base64::prelude::*;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::error::HostError;
use crate::host::{ExternalEvent, WorldHost};
use crate::modes::daemon::ControlMsg;

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

#[derive(Debug, Serialize, Deserialize)]
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
}

impl ControlServer {
    pub fn new<P: Into<std::path::PathBuf>>(
        path: P,
        control_tx: mpsc::Sender<ControlMsg>,
        shutdown_tx: broadcast::Sender<()>,
    ) -> Self {
        let shutdown_rx = shutdown_tx.subscribe();
        Self {
            path: path.into(),
            control_tx,
            shutdown_tx,
            shutdown_rx,
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
                        tokio::spawn(handle_conn(stream, tx, shutdown_tx));
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
) {
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
                error: Some(ControlError {
                    code: "decode_error".into(),
                    message: e.to_string(),
                }),
            },
        };
        if let Ok(json) = serde_json::to_string(&resp) {
            let _ = w.write_all(json.as_bytes()).await;
            let _ = w.write_all(b"\n").await;
        }
        line.clear();
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

/// Minimal control client used by tests and CLI helpers.
pub struct ControlClient {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl ControlClient {
    pub async fn connect(path: &Path) -> std::io::Result<Self> {
        let stream = UnixStream::connect(path).await?;
        let (r, w) = stream.into_split();
        let reader = BufReader::new(r);
        Ok(Self { reader, writer: w })
    }

    pub async fn request(
        &mut self,
        envelope: &RequestEnvelope,
    ) -> std::io::Result<ResponseEnvelope> {
        let json = serde_json::to_string(envelope).unwrap();
        self.writer.write_all(json.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        let mut line = String::new();
        let _ = self.reader.read_line(&mut line).await?;
        let resp = serde_json::from_str(&line)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        Ok(resp)
    }
}

/// Helper for journal-head responses in control server.
pub fn kernel_head(host: &WorldHost<impl aos_store::Store>) -> KernelHeights {
    host.heights()
}
