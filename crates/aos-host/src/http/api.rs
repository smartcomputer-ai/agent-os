use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use base64::prelude::*;
use serde::Deserialize;

use crate::control::ControlError;
use crate::http::{HttpState, control_call};

pub fn router() -> Router<HttpState> {
    Router::new()
        .route("/health", get(health))
        .route("/info", get(info))
        .route("/manifest", get(manifest))
        .route("/defs", get(defs_list))
        .route("/defs/:kind/:name", get(defs_get))
        .route("/state/:reducer", get(state_get))
        .route("/state/:reducer/cells", get(state_cells))
        .route("/events", post(events_post))
        .route("/receipts", post(receipts_post))
        .route("/journal/head", get(journal_head))
        .route("/journal", get(journal_tail))
        .route("/workspace/resolve", get(workspace_resolve))
        .route("/workspace/list", get(workspace_list))
        .route("/workspace/read-ref", get(workspace_read_ref))
        .route("/workspace/read-bytes", get(workspace_read_bytes))
        .route("/workspace/annotations", get(workspace_annotations_get))
        .route("/workspace/write-bytes", post(workspace_write_bytes))
        .route("/workspace/remove", post(workspace_remove))
        .route("/workspace/annotations", post(workspace_annotations_set))
        .route("/workspace/empty-root", post(workspace_empty_root))
        .route("/blob", post(blob_put))
        .route("/blob/:hash", get(blob_get))
        .route("/gov/propose", post(gov_propose))
        .route("/gov/shadow", post(gov_shadow))
        .route("/gov/approve", post(gov_approve))
        .route("/gov/apply", post(gov_apply))
}

#[derive(Debug)]
enum ApiError {
    Control(ControlError),
    Invalid(String),
}

impl ApiError {
    fn bad_request(msg: impl Into<String>) -> Self {
        ApiError::Invalid(msg.into())
    }
}

impl From<ControlError> for ApiError {
    fn from(err: ControlError) -> Self {
        ApiError::Control(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            ApiError::Control(err) => {
                let status = match err.code.as_str() {
                    "invalid_request" | "decode_error" => StatusCode::BAD_REQUEST,
                    "unknown_method" => StatusCode::NOT_FOUND,
                    _ => StatusCode::INTERNAL_SERVER_ERROR,
                };
                (status, err.code, err.message)
            }
            ApiError::Invalid(msg) => (StatusCode::BAD_REQUEST, "invalid_request".into(), msg),
        };
        let body = serde_json::json!({ "code": code, "message": message });
        (status, Json(body)).into_response()
    }
}

async fn health(State(state): State<HttpState>) -> Result<impl IntoResponse, ApiError> {
    let result = control_call(&state, "journal-head", serde_json::json!({})).await?;
    let meta = result
        .get("meta")
        .ok_or_else(|| ApiError::bad_request("missing meta"))?;
    let manifest_hash = meta
        .get("manifest_hash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_request("missing manifest_hash"))?;
    let journal_height = meta
        .get("journal_height")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ApiError::bad_request("missing journal_height"))?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "manifest_hash": manifest_hash,
        "journal_height": journal_height,
    })))
}

async fn info(State(state): State<HttpState>) -> Result<impl IntoResponse, ApiError> {
    let result = control_call(&state, "journal-head", serde_json::json!({})).await?;
    let meta = result
        .get("meta")
        .ok_or_else(|| ApiError::bad_request("missing meta"))?;
    let manifest_hash = meta
        .get("manifest_hash")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let snapshot_hash = meta.get("snapshot_hash").and_then(|v| v.as_str());
    Ok(Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "world_id": serde_json::Value::Null,
        "manifest_hash": manifest_hash,
        "snapshot_hash": snapshot_hash,
    })))
}

#[derive(Debug, Deserialize)]
struct ManifestQuery {
    consistency: Option<String>,
}

async fn manifest(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<ManifestQuery>,
) -> Result<Response, ApiError> {
    let payload = serde_json::json!({
        "consistency": query.consistency.unwrap_or_else(|| "head".into()),
    });
    let result = control_call(&state, "manifest-get", payload).await?;
    let manifest_b64 = result
        .get("manifest_b64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_request("missing manifest_b64"))?;
    let bytes = BASE64_STANDARD
        .decode(manifest_b64)
        .map_err(|e| ApiError::bad_request(format!("invalid base64: {e}")))?;
    if wants_cbor(&headers) {
        return Ok((
            [(axum::http::header::CONTENT_TYPE, "application/cbor")],
            bytes,
        )
            .into_response());
    }
    let manifest: aos_air_types::Manifest = serde_cbor::from_slice(&bytes)
        .map_err(|e| ApiError::bad_request(format!("decode manifest: {e}")))?;
    Ok(Json(serde_json::to_value(manifest).map_err(|e| {
        ApiError::bad_request(format!("encode manifest json: {e}"))
    })?)
    .into_response())
}

#[derive(Debug, Deserialize)]
struct DefsQuery {
    kinds: Option<String>,
    prefix: Option<String>,
}

async fn defs_list(
    State(state): State<HttpState>,
    Query(query): Query<DefsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let kinds = query.kinds.as_ref().map(|raw| {
        raw.split(',')
            .filter(|k| !k.is_empty())
            .map(|k| k.to_string())
            .collect::<Vec<_>>()
    });
    let payload = serde_json::json!({
        "kinds": kinds,
        "prefix": query.prefix,
    });
    let result = control_call(&state, "defs-list", payload).await?;
    Ok(Json(result))
}

async fn defs_get(
    State(state): State<HttpState>,
    Path((kind, name)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let payload = serde_json::json!({
        "name": format!("{kind}/{name}"),
    });
    let result = control_call(&state, "def-get", payload).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct StateQuery {
    key_b64: Option<String>,
    consistency: Option<String>,
}

async fn state_get(
    State(state): State<HttpState>,
    Path(reducer): Path<String>,
    Query(query): Query<StateQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let payload = serde_json::json!({
        "reducer": reducer,
        "key_b64": query.key_b64,
        "consistency": query.consistency,
    });
    let result = control_call(&state, "state-get", payload).await?;
    Ok(Json(result))
}

async fn state_cells(
    State(state): State<HttpState>,
    Path(reducer): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let payload = serde_json::json!({ "reducer": reducer });
    let result = control_call(&state, "state-list", payload).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct EventPayload {
    schema: String,
    #[serde(default)]
    value: Option<serde_json::Value>,
    #[serde(default)]
    value_b64: Option<String>,
    #[serde(default)]
    key_b64: Option<String>,
}

async fn events_post(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let payload: EventPayload = parse_body(&headers, &body)?;
    let value_b64 = match (payload.value_b64, payload.value) {
        (Some(b64), _) => b64,
        (None, Some(value)) => {
            let bytes = serde_cbor::to_vec(&value)
                .map_err(|e| ApiError::bad_request(format!("encode cbor: {e}")))?;
            BASE64_STANDARD.encode(bytes)
        }
        _ => return Err(ApiError::bad_request("missing value or value_b64")),
    };
    let payload = serde_json::json!({
        "schema": payload.schema,
        "value_b64": value_b64,
        "key_b64": payload.key_b64,
    });
    let result = control_call(&state, "event-send", payload).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct ReceiptPayload {
    intent_hash: String,
    adapter_id: String,
    #[serde(default)]
    payload: Option<serde_json::Value>,
    #[serde(default)]
    payload_b64: Option<String>,
}

async fn receipts_post(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let payload: ReceiptPayload = parse_body(&headers, &body)?;
    let payload_b64 = match (payload.payload_b64, payload.payload) {
        (Some(b64), _) => b64,
        (None, Some(value)) => {
            let bytes = serde_cbor::to_vec(&value)
                .map_err(|e| ApiError::bad_request(format!("encode cbor: {e}")))?;
            BASE64_STANDARD.encode(bytes)
        }
        _ => return Err(ApiError::bad_request("missing payload or payload_b64")),
    };
    let hash = decode_hash_hex(&payload.intent_hash)?;
    let payload = serde_json::json!({
        "intent_hash": hash_to_json_array(&hash),
        "adapter_id": payload.adapter_id,
        "payload_b64": payload_b64,
    });
    let result = control_call(&state, "receipt-inject", payload).await?;
    Ok(Json(result))
}

async fn journal_head(State(state): State<HttpState>) -> Result<impl IntoResponse, ApiError> {
    let result = control_call(&state, "journal-head", serde_json::json!({})).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct JournalQuery {
    from: Option<u64>,
    limit: Option<u64>,
}

async fn journal_tail(
    State(state): State<HttpState>,
    Query(query): Query<JournalQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let payload = serde_json::json!({
        "from": query.from.unwrap_or(0),
        "limit": query.limit,
    });
    let result = control_call(&state, "journal-list", payload).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct WorkspaceResolveQuery {
    workspace: String,
    version: Option<u64>,
}

async fn workspace_resolve(
    State(state): State<HttpState>,
    Query(query): Query<WorkspaceResolveQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let payload = serde_json::json!({
        "workspace": query.workspace,
        "version": query.version,
    });
    let result = control_call(&state, "workspace-resolve", payload).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct WorkspaceListQuery {
    root_hash: String,
    path: Option<String>,
    scope: Option<String>,
    cursor: Option<String>,
    limit: Option<u64>,
}

async fn workspace_list(
    State(state): State<HttpState>,
    Query(query): Query<WorkspaceListQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let payload = serde_json::json!({
        "root_hash": query.root_hash,
        "path": query.path,
        "scope": query.scope,
        "cursor": query.cursor,
        "limit": query.limit.unwrap_or(1000),
    });
    let result = control_call(&state, "workspace-list", payload).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct WorkspaceReadRefQuery {
    root_hash: String,
    path: String,
}

async fn workspace_read_ref(
    State(state): State<HttpState>,
    Query(query): Query<WorkspaceReadRefQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let payload = serde_json::json!({
        "root_hash": query.root_hash,
        "path": query.path,
    });
    let result = control_call(&state, "workspace-read-ref", payload).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct WorkspaceReadBytesQuery {
    root_hash: String,
    path: String,
    range: Option<String>,
}

async fn workspace_read_bytes(
    State(state): State<HttpState>,
    Query(query): Query<WorkspaceReadBytesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let range = query
        .range
        .as_deref()
        .and_then(parse_range)
        .map(|(start, end)| serde_json::json!({ "start": start, "end": end }));
    let payload = serde_json::json!({
        "root_hash": query.root_hash,
        "path": query.path,
        "range": range,
    });
    let result = control_call(&state, "workspace-read-bytes", payload).await?;
    let data_b64 = result
        .get("data_b64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_request("missing data_b64"))?;
    let bytes = BASE64_STANDARD
        .decode(data_b64)
        .map_err(|e| ApiError::bad_request(format!("invalid base64: {e}")))?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    ))
}

#[derive(Debug, Deserialize)]
struct WorkspaceAnnotationsQuery {
    root_hash: String,
    path: Option<String>,
}

async fn workspace_annotations_get(
    State(state): State<HttpState>,
    Query(query): Query<WorkspaceAnnotationsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let payload = serde_json::json!({
        "root_hash": query.root_hash,
        "path": query.path,
    });
    let result = control_call(&state, "workspace-annotations-get", payload).await?;
    Ok(Json(result))
}

async fn workspace_write_bytes(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let payload = parse_body::<serde_json::Value>(&headers, &body)?;
    let result = control_call(&state, "workspace-write-bytes", payload).await?;
    Ok(Json(result))
}

async fn workspace_remove(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let payload = parse_body::<serde_json::Value>(&headers, &body)?;
    let result = control_call(&state, "workspace-remove", payload).await?;
    Ok(Json(result))
}

async fn workspace_annotations_set(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let payload = parse_body::<serde_json::Value>(&headers, &body)?;
    let result = control_call(&state, "workspace-annotations-set", payload).await?;
    Ok(Json(result))
}

async fn workspace_empty_root(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let payload = parse_body::<serde_json::Value>(&headers, &body)?;
    let result = control_call(&state, "workspace-empty-root", payload).await?;
    Ok(Json(result))
}

async fn blob_put(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let payload = parse_body::<serde_json::Value>(&headers, &body)?;
    let result = control_call(&state, "blob-put", payload).await?;
    Ok(Json(result))
}

async fn blob_get(
    State(state): State<HttpState>,
    Path(hash): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let payload = serde_json::json!({ "hash_hex": hash });
    let result = control_call(&state, "blob-get", payload).await?;
    let data_b64 = result
        .get("data_b64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_request("missing data_b64"))?;
    let bytes = BASE64_STANDARD
        .decode(data_b64)
        .map_err(|e| ApiError::bad_request(format!("invalid base64: {e}")))?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    ))
}

async fn gov_propose(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let payload = parse_body::<serde_json::Value>(&headers, &body)?;
    let result = control_call(&state, "gov-propose", payload).await?;
    Ok(Json(result))
}

async fn gov_shadow(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let payload = parse_body::<serde_json::Value>(&headers, &body)?;
    let result = control_call(&state, "gov-shadow", payload).await?;
    Ok(Json(result))
}

async fn gov_approve(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let payload = parse_body::<serde_json::Value>(&headers, &body)?;
    let result = control_call(&state, "gov-approve", payload).await?;
    Ok(Json(result))
}

async fn gov_apply(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let payload = parse_body::<serde_json::Value>(&headers, &body)?;
    let result = control_call(&state, "gov-apply", payload).await?;
    Ok(Json(result))
}

fn parse_body<T: serde::de::DeserializeOwned>(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<T, ApiError> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json");
    if content_type.starts_with("application/cbor") {
        serde_cbor::from_slice(body)
            .map_err(|e| ApiError::bad_request(format!("decode cbor: {e}")))
    } else {
        serde_json::from_slice(body)
            .map_err(|e| ApiError::bad_request(format!("decode json: {e}")))
    }
}

fn wants_cbor(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("application/cbor"))
        .unwrap_or(false)
}

fn parse_range(raw: &str) -> Option<(u64, u64)> {
    let (start, end) = raw.split_once('-')?;
    let start = start.parse().ok()?;
    let end = end.parse().ok()?;
    Some((start, end))
}

fn decode_hash_hex(raw: &str) -> Result<[u8; 32], ApiError> {
    let bytes = hex::decode(raw)
        .map_err(|e| ApiError::bad_request(format!("invalid hash hex: {e}")))?;
    if bytes.len() != 32 {
        return Err(ApiError::bad_request("intent_hash must be 32 bytes"));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn hash_to_json_array(hash: &[u8; 32]) -> serde_json::Value {
    serde_json::Value::Array(hash.iter().map(|b| serde_json::json!(*b)).collect())
}
