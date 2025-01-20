use std::collections::{BTreeMap, HashMap, HashSet};

use aos_sys::{HttpPublishRegistry, HttpPublishRule};
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::prelude::*;
use serde::Deserialize;

#[cfg(test)]
use aos_sys::WorkspaceRef;

use crate::http::{HttpState, control_call};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedPath {
    pub path: String,
    pub segments: Vec<String>,
    pub had_trailing_slash: bool,
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    MissingLeadingSlash,
    InvalidPercentEncoding,
    InvalidUtf8,
    InvalidSegment(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchError {
    RequestPath(PathError),
    RulePrefix { id: String, error: PathError },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeError {
    Invalid,
    Unsatisfiable,
}

#[derive(Debug, Clone, Copy)]
pub struct PrefixRule<'a, T> {
    pub id: &'a str,
    pub rule: &'a T,
    pub prefix: &'a NormalizedPath,
}

pub fn normalize_request_path(raw: &str) -> Result<NormalizedPath, PathError> {
    normalize_path(raw, true)
}

pub fn normalize_route_prefix(raw: &str) -> Result<NormalizedPath, PathError> {
    normalize_path(raw, true)
}

pub fn prefix_matches(prefix: &NormalizedPath, path: &NormalizedPath) -> bool {
    let prefix_len = prefix.segments.len();
    if prefix_len > path.segments.len() {
        return false;
    }
    prefix
        .segments
        .iter()
        .zip(path.segments.iter())
        .all(|(left, right)| left == right)
}

pub fn select_longest_prefix<'a, T>(
    path: &NormalizedPath,
    rules: impl IntoIterator<Item = PrefixRule<'a, T>>,
) -> Option<PrefixRule<'a, T>> {
    let mut best: Option<PrefixRule<'a, T>> = None;
    let mut best_len = 0usize;
    for candidate in rules {
        if !prefix_matches(candidate.prefix, path) {
            continue;
        }
        let len = candidate.prefix.segments.len();
        if best.is_none() || len > best_len {
            best = Some(candidate);
            best_len = len;
        }
    }
    best
}

#[derive(Debug, Clone)]
pub struct PublishMatch<'a> {
    pub rule_id: &'a str,
    pub rule: &'a HttpPublishRule,
    pub request: NormalizedPath,
    pub suffix_segments: Vec<String>,
    pub suffix: String,
}

pub fn match_publish_rule<'a>(
    rules: &'a BTreeMap<String, HttpPublishRule>,
    request_path: &str,
) -> Result<Option<PublishMatch<'a>>, MatchError> {
    let request = normalize_request_path(request_path).map_err(MatchError::RequestPath)?;
    let mut best: Option<PublishMatch<'a>> = None;
    let mut best_len = 0usize;
    for (id, rule) in rules {
        let prefix = normalize_route_prefix(&rule.route_prefix)
            .map_err(|error| MatchError::RulePrefix {
                id: id.clone(),
                error,
            })?;
        if !prefix_matches(&prefix, &request) {
            continue;
        }
        let len = prefix.segments.len();
        if best.is_none() || len > best_len {
            let suffix_segments = request.segments[len..].to_vec();
            let suffix = suffix_segments.join("/");
            best = Some(PublishMatch {
                rule_id: id.as_str(),
                rule,
                request: request.clone(),
                suffix_segments,
                suffix,
            });
            best_len = len;
        }
    }
    Ok(best)
}

pub fn join_workspace_path(base: Option<&str>, suffix: &str) -> String {
    let base = base.unwrap_or("").trim_matches('/');
    if base.is_empty() {
        return suffix.to_string();
    }
    if suffix.is_empty() {
        return base.to_string();
    }
    format!("{base}/{suffix}")
}

pub async fn handler(
    State(state): State<HttpState>,
    req: axum::http::Request<Body>,
) -> Response {
    match handle_publish(state, req).await {
        Ok(resp) => resp,
        Err(err) => err.into_response(),
    }
}

async fn handle_publish(
    state: HttpState,
    req: axum::http::Request<Body>,
) -> Result<Response, PublishError> {
    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or_else(|| req.uri().path());
    let request_headers = req.headers().clone();
    if path.starts_with("/api") {
        return Err(PublishError::not_found());
    }
    let registry = load_registry(&state).await?;
    let Some(registry) = registry else {
        return Err(PublishError::not_found());
    };
    let matched = match match_publish_rule(&registry.rules, path) {
        Ok(Some(matched)) => Some(matched),
        Ok(None) => None,
        Err(MatchError::RequestPath(_)) => {
            if accepts_html(&request_headers) {
                if let Some(rule) = match_rule_by_raw_path(&registry.rules, req.uri().path()) {
                    return serve_default_doc_for_rule(&state, rule, &request_headers).await;
                }
            }
            return Err(PublishError::not_found());
        }
        Err(err) => {
            return Err(PublishError::invalid(format!(
                "match publish rule: {err:?}"
            )));
        }
    };
    let Some(matched) = matched else {
        return Err(PublishError::not_found());
    };
    let rule = matched.rule;
    if rule.cache == "immutable" && rule.workspace.version.is_none() {
        return Err(PublishError::invalid(
            "cache=immutable requires pinned workspace.version",
        ));
    }

    let base_path = rule.workspace.path.as_deref();
    let target_path = join_workspace_path(base_path, &matched.suffix);
    let resolve = workspace_resolve(&state, &rule.workspace).await?;
    if !resolve.exists {
        return Err(PublishError::not_found());
    }
    let root_hash = resolve
        .root_hash
        .ok_or_else(|| PublishError::invalid("workspace resolve missing root_hash"))?;

    let (entry, entry_path) = match resolve_entry(
        &state,
        &root_hash,
        &target_path,
    )
    .await?
    {
        Some(entry) => entry,
        None => {
            if accepts_html(&request_headers) {
                return serve_default_doc_for_rule(&state, rule, &request_headers).await;
            }
            return Err(PublishError::not_found());
        }
    };

    if entry.kind == "file" {
        if matched.request.had_trailing_slash {
            return Err(PublishError::not_found());
        }
        return serve_file(
            &state,
            &root_hash,
            &entry_path,
            &entry,
            rule,
            &request_headers,
            false,
        )
        .await;
    }

    if entry.kind == "dir" {
        if !matched.request.had_trailing_slash {
            return Ok(redirect_with_slash(&matched.request));
        }
        if let Some(default_doc) = rule.default_doc.as_deref() {
            let doc_path = join_workspace_path(Some(&entry_path), default_doc);
            if let Some(doc_entry) =
                workspace_read_ref(&state, &root_hash, &doc_path).await?
            {
                if doc_entry.kind == "file" {
                    return serve_file(
                        &state,
                        &root_hash,
                        &doc_path,
                        &doc_entry,
                        rule,
                        &request_headers,
                        true,
                    )
                    .await;
                }
            }
        }
        if rule.allow_dir_listing {
            let listing =
                workspace_list(&state, &root_hash, Some(&entry_path)).await?;
            return Ok((
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                serde_json::to_vec(&listing).map_err(|e| {
                    PublishError::invalid(format!("encode listing: {e}"))
                })?,
            )
                .into_response());
        }
    }

    Err(PublishError::not_found())
}

async fn serve_file(
    state: &HttpState,
    root_hash: &str,
    path: &str,
    entry: &WorkspaceRefEntry,
    rule: &HttpPublishRule,
    request_headers: &HeaderMap,
    is_default: bool,
) -> Result<Response, PublishError> {
    let range = match parse_range_header(request_headers, entry.size) {
        Ok(range) => range,
        Err(RangeError::Invalid) => return Ok(range_invalid_response()),
        Err(RangeError::Unsatisfiable) => return Ok(range_unsatisfiable_response(entry.size)),
    };
    let bytes = workspace_read_bytes(state, root_hash, path, range).await?;
    let mut headers = resolve_headers(state, root_hash, path).await?;
    apply_cache_headers(rule, &entry.hash, &mut headers);
    headers.insert(
        HeaderName::from_static("etag"),
        HeaderValue::from_str(&entry.hash)
            .map_err(|_| PublishError::invalid("invalid etag value"))?,
    );
    headers.insert(
        HeaderName::from_static("content-length"),
        HeaderValue::from_str(&bytes.len().to_string()).unwrap_or_else(|_| {
            HeaderValue::from_static("0")
        }),
    );
    if !headers.contains_key(HeaderName::from_static("content-type")) {
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/octet-stream"),
        );
    }
    headers.insert(
        HeaderName::from_static("x-aos-root-hash"),
        HeaderValue::from_str(root_hash)
            .map_err(|_| PublishError::invalid("invalid x-aos-root-hash header"))?,
    );
    headers.insert(
        header::ACCEPT_RANGES,
        HeaderValue::from_static("bytes"),
    );
    let status = if let Some((start, end)) = range {
        let end_inclusive = end.saturating_sub(1);
        let value = format!("bytes {start}-{end_inclusive}/{}", entry.size);
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&value)
                .map_err(|_| PublishError::invalid("invalid content-range header"))?,
        );
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };

    let mut resp = Response::new(Body::from(bytes));
    *resp.status_mut() = status;
    *resp.headers_mut() = headers;
    if is_default {
        resp.headers_mut().insert(
            HeaderName::from_static("x-aos-default-doc"),
            HeaderValue::from_static("true"),
        );
    }
    Ok(resp)
}

fn redirect_with_slash(request: &NormalizedPath) -> Response {
    let mut location = if request.path == "/" {
        "/".to_string()
    } else {
        format!("{}/", request.path)
    };
    if let Some(query) = &request.query {
        location.push('?');
        location.push_str(query);
    }
    (
        StatusCode::PERMANENT_REDIRECT,
        [(axum::http::header::LOCATION, location)],
    )
        .into_response()
}

async fn load_registry(
    state: &HttpState,
) -> Result<Option<HttpPublishRegistry>, PublishError> {
    let result = control_call(
        state,
        "state-get",
        serde_json::json!({
            "reducer": "sys/HttpPublish@1",
        }),
    )
    .await
    .map_err(|err| PublishError::invalid(err.message))?;
    let state_b64 = result.get("state_b64").and_then(|v| v.as_str());
    let Some(state_b64) = state_b64 else {
        return Ok(None);
    };
    let bytes = BASE64_STANDARD
        .decode(state_b64)
        .map_err(|e| PublishError::invalid(format!("decode state: {e}")))?;
    let registry: HttpPublishRegistry = serde_cbor::from_slice(&bytes)
        .map_err(|e| PublishError::invalid(format!("decode registry: {e}")))?;
    Ok(Some(registry))
}

async fn resolve_entry(
    state: &HttpState,
    root_hash: &str,
    target_path: &str,
) -> Result<Option<(WorkspaceRefEntry, String)>, PublishError> {
    if target_path.is_empty() {
        return Ok(Some((
            WorkspaceRefEntry {
                kind: "dir".to_string(),
                hash: root_hash.to_string(),
                size: 0,
                mode: 0,
            },
            "".to_string(),
        )));
    }
    if let Some(entry) = workspace_read_ref(state, root_hash, target_path).await? {
        return Ok(Some((entry, target_path.to_string())));
    }
    Ok(None)
}

async fn resolve_headers(
    state: &HttpState,
    root_hash: &str,
    path: &str,
) -> Result<HeaderMap, PublishError> {
    let mut headers = HeaderMap::new();
    let mut remaining = HashSet::from([
        "http.content-type",
        "http.content-encoding",
        "http.content-language",
        "http.content-disposition",
        "http.cache-control",
    ]);
    let mut paths = Vec::new();
    let mut cursor = if path.is_empty() { None } else { Some(path.to_string()) };
    loop {
        paths.push(cursor.clone());
        match cursor {
            Some(current) => cursor = parent_path(&current),
            None => break,
        }
    }

    for path in paths {
        if remaining.is_empty() {
            break;
        }
        let annotations = workspace_annotations_get(state, root_hash, path.as_deref()).await?;
        if let Some(map) = annotations {
            for (key, hash) in map {
                if !remaining.contains(key.as_str()) {
                    continue;
                }
                let bytes = blob_get(state, &hash).await?;
                if let Ok(value) = std::str::from_utf8(&bytes) {
                    let header_name = key.strip_prefix("http.").unwrap_or(&key);
                    if let Ok(name) = HeaderName::from_bytes(header_name.as_bytes()) {
                        if let Ok(header_value) = HeaderValue::from_str(value) {
                            headers.insert(name, header_value);
                            remaining.remove(key.as_str());
                        }
                    }
                }
            }
        }
    }
    Ok(headers)
}

fn parent_path(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = path.split('/').collect();
    if parts.is_empty() {
        return None;
    }
    parts.pop();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn apply_cache_headers(rule: &HttpPublishRule, entry_hash: &str, headers: &mut HeaderMap) {
    match rule.cache.as_str() {
        "immutable" => {
            headers.insert(
                HeaderName::from_static("cache-control"),
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            );
        }
        "etag" => {
            if !headers.contains_key(HeaderName::from_static("cache-control")) {
                headers.insert(
                    HeaderName::from_static("cache-control"),
                    HeaderValue::from_static("no-cache"),
                );
            }
        }
        _ => {}
    }
    if !headers.contains_key(HeaderName::from_static("etag")) {
        if let Ok(value) = HeaderValue::from_str(entry_hash) {
            headers.insert(HeaderName::from_static("etag"), value);
        }
    }
}

fn match_rule_by_raw_path<'a>(
    rules: &'a BTreeMap<String, HttpPublishRule>,
    raw_path: &str,
) -> Option<&'a HttpPublishRule> {
    let mut best: Option<&HttpPublishRule> = None;
    let mut best_len = 0usize;
    for rule in rules.values() {
        let prefix = rule.route_prefix.as_str();
        if !raw_prefix_matches(prefix, raw_path) {
            continue;
        }
        let len = prefix.trim_end_matches('/').len();
        if best.is_none() || len > best_len {
            best = Some(rule);
            best_len = len;
        }
    }
    best
}

fn raw_prefix_matches(prefix: &str, raw_path: &str) -> bool {
    let prefix = if prefix == "/" {
        "/"
    } else {
        prefix.trim_end_matches('/')
    };
    if prefix == "/" {
        return raw_path.starts_with('/');
    }
    raw_path == prefix || raw_path.starts_with(&format!("{prefix}/"))
}

async fn serve_default_doc_for_rule(
    state: &HttpState,
    rule: &HttpPublishRule,
    request_headers: &HeaderMap,
) -> Result<Response, PublishError> {
    let Some(default_doc) = rule.default_doc.as_deref() else {
        return Err(PublishError::not_found());
    };
    let base_path = rule.workspace.path.as_deref();
    let resolve = workspace_resolve(state, &rule.workspace).await?;
    if !resolve.exists {
        return Err(PublishError::not_found());
    }
    let root_hash = resolve
        .root_hash
        .ok_or_else(|| PublishError::invalid("workspace resolve missing root_hash"))?;
    let doc_path = join_workspace_path(base_path, default_doc);
    if let Some(doc_entry) = workspace_read_ref(state, &root_hash, &doc_path).await? {
        if doc_entry.kind == "file" {
            return serve_file(
                state,
                &root_hash,
                &doc_path,
                &doc_entry,
                rule,
                request_headers,
                true,
            )
            .await;
        }
    }
    Err(PublishError::not_found())
}

fn accepts_html(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(header::ACCEPT) else {
        return false;
    };
    let Ok(text) = value.to_str() else {
        return false;
    };
    let text = text.to_ascii_lowercase();
    text.contains("text/html") || text.contains("*/*")
}

fn parse_range_header(
    headers: &HeaderMap,
    size: u64,
) -> Result<Option<(u64, u64)>, RangeError> {
    let value = match headers.get(header::RANGE) {
        Some(value) => value,
        None => return Ok(None),
    };
    let value = value.to_str().map_err(|_| RangeError::Invalid)?;
    let value = value.trim();
    let Some(range_spec) = value.strip_prefix("bytes=") else {
        return Ok(None);
    };
    if range_spec.contains(',') {
        return Err(RangeError::Invalid);
    }
    let (start_str, end_str) = range_spec
        .split_once('-')
        .ok_or(RangeError::Invalid)?;
    if start_str.is_empty() {
        let suffix: u64 = end_str.parse().map_err(|_| RangeError::Invalid)?;
        if suffix == 0 {
            return Err(RangeError::Invalid);
        }
        if suffix >= size {
            return Ok(Some((0, size)));
        }
        return Ok(Some((size - suffix, size)));
    }
    let start: u64 = start_str.parse().map_err(|_| RangeError::Invalid)?;
    if start >= size {
        return Err(RangeError::Unsatisfiable);
    }
    if end_str.is_empty() {
        return Ok(Some((start, size)));
    }
    let end_inclusive: u64 = end_str.parse().map_err(|_| RangeError::Invalid)?;
    if end_inclusive < start {
        return Err(RangeError::Invalid);
    }
    let end_exclusive = end_inclusive
        .saturating_add(1)
        .min(size);
    Ok(Some((start, end_exclusive)))
}

fn range_invalid_response() -> Response {
    (StatusCode::BAD_REQUEST, "invalid range").into_response()
}

fn range_unsatisfiable_response(size: u64) -> Response {
    let mut resp = Response::new(Body::from("range not satisfiable"));
    *resp.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
    let value = format!("bytes */{size}");
    if let Ok(header_value) = HeaderValue::from_str(&value) {
        resp.headers_mut()
            .insert(header::CONTENT_RANGE, header_value);
    }
    resp.headers_mut()
        .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    resp
}

async fn workspace_resolve(
    state: &HttpState,
    reference: &aos_sys::WorkspaceRef,
) -> Result<WorkspaceResolveReceipt, PublishError> {
    let result = control_call(
        state,
        "workspace-resolve",
        serde_json::json!({
            "workspace": reference.workspace.as_str(),
            "version": reference.version,
        }),
    )
    .await
    .map_err(|err| PublishError::invalid(err.message))?;
    serde_json::from_value(result)
        .map_err(|e| PublishError::invalid(format!("decode resolve: {e}")))
}

async fn workspace_read_ref(
    state: &HttpState,
    root_hash: &str,
    path: &str,
) -> Result<Option<WorkspaceRefEntry>, PublishError> {
    let result = control_call(
        state,
        "workspace-read-ref",
        serde_json::json!({
            "root_hash": root_hash,
            "path": path,
        }),
    )
    .await
    .map_err(|err| PublishError::invalid(err.message))?;
    serde_json::from_value(result)
        .map_err(|e| PublishError::invalid(format!("decode read_ref: {e}")))
}

async fn workspace_read_bytes(
    state: &HttpState,
    root_hash: &str,
    path: &str,
    range: Option<(u64, u64)>,
) -> Result<Vec<u8>, PublishError> {
    let range_val = range.map(|(start, end)| serde_json::json!({ "start": start, "end": end }));
    let result = control_call(
        state,
        "workspace-read-bytes",
        serde_json::json!({
            "root_hash": root_hash,
            "path": path,
            "range": range_val,
        }),
    )
    .await
    .map_err(|err| PublishError::invalid(err.message))?;
    let data_b64 = result
        .get("data_b64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PublishError::invalid("missing data_b64"))?;
    BASE64_STANDARD
        .decode(data_b64)
        .map_err(|e| PublishError::invalid(format!("decode bytes: {e}")))
}

async fn workspace_list(
    state: &HttpState,
    root_hash: &str,
    path: Option<&str>,
) -> Result<WorkspaceListReceipt, PublishError> {
    let result = control_call(
        state,
        "workspace-list",
        serde_json::json!({
            "root_hash": root_hash,
            "path": path,
            "scope": "dir",
            "limit": 1000,
        }),
    )
    .await
    .map_err(|err| PublishError::invalid(err.message))?;
    serde_json::from_value(result)
        .map_err(|e| PublishError::invalid(format!("decode list: {e}")))
}

async fn workspace_annotations_get(
    state: &HttpState,
    root_hash: &str,
    path: Option<&str>,
) -> Result<Option<HashMap<String, String>>, PublishError> {
    let result = control_call(
        state,
        "workspace-annotations-get",
        serde_json::json!({
            "root_hash": root_hash,
            "path": path,
        }),
    )
    .await
    .map_err(|err| PublishError::invalid(err.message))?;
    let receipt: WorkspaceAnnotationsGetReceipt = serde_json::from_value(result)
        .map_err(|e| PublishError::invalid(format!("decode annotations: {e}")))?;
    Ok(receipt.annotations)
}

async fn blob_get(state: &HttpState, hash: &str) -> Result<Vec<u8>, PublishError> {
    let result = control_call(
        state,
        "blob-get",
        serde_json::json!({
            "hash_hex": hash,
        }),
    )
    .await
    .map_err(|err| PublishError::invalid(err.message))?;
    let data_b64 = result
        .get("data_b64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PublishError::invalid("missing data_b64"))?;
    BASE64_STANDARD
        .decode(data_b64)
        .map_err(|e| PublishError::invalid(format!("decode blob: {e}")))
}

#[derive(Debug, Deserialize)]
struct WorkspaceResolveReceipt {
    exists: bool,
    #[serde(default)]
    root_hash: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct WorkspaceRefEntry {
    kind: String,
    hash: String,
    size: u64,
    mode: u64,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct WorkspaceListReceipt {
    entries: Vec<WorkspaceListEntry>,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct WorkspaceListEntry {
    path: String,
    kind: String,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    mode: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceAnnotationsGetReceipt {
    annotations: Option<HashMap<String, String>>,
}

#[derive(Debug)]
struct PublishError {
    status: StatusCode,
    message: String,
}

impl PublishError {
    fn invalid(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
        }
    }

    fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: "not found".into(),
        }
    }
}

impl IntoResponse for PublishError {
    fn into_response(self) -> Response {
        (self.status, self.message).into_response()
    }
}

fn normalize_path(raw: &str, require_leading_slash: bool) -> Result<NormalizedPath, PathError> {
    let raw = raw.split('#').next().unwrap_or("");
    let (raw_path, query) = match raw.split_once('?') {
        Some((path, query)) => (path, Some(query.to_string())),
        None => (raw, None),
    };
    let raw_path = if raw_path.is_empty() { "/" } else { raw_path };
    let decoded = percent_decode(raw_path)?;
    if require_leading_slash && !decoded.starts_with('/') {
        return Err(PathError::MissingLeadingSlash);
    }
    let collapsed = collapse_slashes(&decoded);
    let had_trailing_slash = collapsed.ends_with('/');
    let normalized = if collapsed != "/" && had_trailing_slash {
        collapsed.trim_end_matches('/').to_string()
    } else {
        collapsed
    };
    let segments = if normalized == "/" {
        Vec::new()
    } else {
        normalized
            .trim_start_matches('/')
            .split('/')
            .map(|segment| {
                if is_valid_segment(segment) {
                    Ok(segment.to_string())
                } else {
                    Err(PathError::InvalidSegment(segment.to_string()))
                }
            })
            .collect::<Result<Vec<String>, PathError>>()?
    };
    Ok(NormalizedPath {
        path: normalized,
        segments,
        had_trailing_slash,
        query,
    })
}

fn percent_decode(input: &str) -> Result<String, PathError> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        let byte = bytes[idx];
        if byte == b'%' {
            if idx + 2 >= bytes.len() {
                return Err(PathError::InvalidPercentEncoding);
            }
            let hi = hex_value(bytes[idx + 1])?;
            let lo = hex_value(bytes[idx + 2])?;
            out.push((hi << 4) | lo);
            idx += 3;
        } else {
            out.push(byte);
            idx += 1;
        }
    }
    String::from_utf8(out).map_err(|_| PathError::InvalidUtf8)
}

fn hex_value(byte: u8) -> Result<u8, PathError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(PathError::InvalidPercentEncoding),
    }
}

fn collapse_slashes(input: &str) -> String {
    let mut out = String::new();
    let mut last_was_slash = false;
    for ch in input.chars() {
        if ch == '/' {
            if !last_was_slash {
                out.push('/');
                last_was_slash = true;
            }
        } else {
            out.push(ch);
            last_was_slash = false;
        }
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

fn is_valid_segment(segment: &str) -> bool {
    if segment.is_empty() {
        return false;
    }
    if segment == "." || segment == ".." {
        return false;
    }
    segment.chars().all(is_url_safe_char)
}

fn is_url_safe_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '~' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_query_and_fragment() {
        let normalized = normalize_request_path("/app/index.html?x=1#y").unwrap();
        assert_eq!(normalized.path, "/app/index.html");
        assert_eq!(normalized.query, Some("x=1".to_string()));
        assert_eq!(
            normalized.segments,
            vec!["app".to_string(), "index.html".to_string()]
        );
    }

    #[test]
    fn normalize_collapses_and_trims_slashes() {
        let normalized = normalize_request_path("//foo///bar/").unwrap();
        assert_eq!(normalized.path, "/foo/bar");
        assert!(normalized.had_trailing_slash);
        assert_eq!(
            normalized.segments,
            vec!["foo".to_string(), "bar".to_string()]
        );
    }

    #[test]
    fn normalize_rejects_invalid_percent_encoding() {
        let err = normalize_request_path("/foo/%2").unwrap_err();
        assert_eq!(err, PathError::InvalidPercentEncoding);
    }

    #[test]
    fn normalize_rejects_invalid_segment_chars() {
        let err = normalize_request_path("/foo/bar$").unwrap_err();
        assert_eq!(err, PathError::InvalidSegment("bar$".to_string()));
    }

    #[test]
    fn normalize_rejects_dot_segments() {
        let err = normalize_request_path("/foo/..").unwrap_err();
        assert_eq!(err, PathError::InvalidSegment("..".to_string()));
    }

    #[test]
    fn normalize_allows_root() {
        let normalized = normalize_request_path("/").unwrap();
        assert_eq!(normalized.path, "/");
        assert!(normalized.segments.is_empty());
    }

    #[test]
    fn normalize_route_prefix_requires_leading_slash() {
        let err = normalize_route_prefix("app").unwrap_err();
        assert_eq!(err, PathError::MissingLeadingSlash);
    }

    #[test]
    fn prefix_matches_by_segment() {
        let prefix = normalize_route_prefix("/app").unwrap();
        let exact = normalize_request_path("/app").unwrap();
        let child = normalize_request_path("/app/assets/logo.png").unwrap();
        let sibling = normalize_request_path("/apple").unwrap();
        assert!(prefix_matches(&prefix, &exact));
        assert!(prefix_matches(&prefix, &child));
        assert!(!prefix_matches(&prefix, &sibling));
    }

    #[test]
    fn select_longest_prefix_prefers_longer_match() {
        let root = normalize_route_prefix("/").unwrap();
        let app = normalize_route_prefix("/app").unwrap();
        let assets = normalize_route_prefix("/app/assets").unwrap();
        let path = normalize_request_path("/app/assets/logo.png").unwrap();
        let rules = vec![
            PrefixRule {
                id: "root",
                rule: &1,
                prefix: &root,
            },
            PrefixRule {
                id: "app",
                rule: &2,
                prefix: &app,
            },
            PrefixRule {
                id: "assets",
                rule: &3,
                prefix: &assets,
            },
        ];
        let matched = select_longest_prefix(&path, rules).expect("match");
        assert_eq!(matched.id, "assets");
        assert_eq!(*matched.rule, 3);
    }

    #[test]
    fn match_publish_rule_returns_suffix_and_trailing_slash() {
        let rules = publish_rules(vec![
            ("/", "root"),
            ("/app", "app"),
            ("/app/assets", "assets"),
        ]);
        let matched = match_publish_rule(&rules, "/app/assets/logo.png?x=1")
            .unwrap()
            .expect("match");
        assert_eq!(matched.rule_id, "assets");
        assert_eq!(
            matched.suffix_segments,
            vec!["logo.png".to_string()]
        );
        assert_eq!(matched.suffix, "logo.png");
        assert_eq!(matched.request.query, Some("x=1".to_string()));
    }

    #[test]
    fn match_publish_rule_rejects_invalid_prefix() {
        let mut rules = publish_rules(vec![("/", "root")]);
        let mut bad = HttpPublishRule {
            route_prefix: "app".to_string(),
            workspace: workspace_ref("shell"),
            default_doc: None,
            allow_dir_listing: false,
            cache: "etag".to_string(),
        };
        rules.insert("bad".to_string(), bad.clone());
        let err = match_publish_rule(&rules, "/app").unwrap_err();
        assert_eq!(
            err,
            MatchError::RulePrefix {
                id: "bad".to_string(),
                error: PathError::MissingLeadingSlash
            }
        );
        bad.route_prefix = "/ok".to_string();
        rules.insert("bad".to_string(), bad);
        let matched = match_publish_rule(&rules, "/ok/").unwrap().expect("match");
        assert_eq!(matched.rule_id, "bad");
        assert!(matched.request.had_trailing_slash);
    }

    #[test]
    fn join_workspace_path_handles_empty_components() {
        assert_eq!(join_workspace_path(None, "index.html"), "index.html");
        assert_eq!(join_workspace_path(Some("app"), ""), "app");
        assert_eq!(join_workspace_path(Some("app"), "index.html"), "app/index.html");
        assert_eq!(join_workspace_path(Some("/app/"), "assets/logo.png"), "app/assets/logo.png");
    }

    fn publish_rules(entries: Vec<(&str, &str)>) -> BTreeMap<String, HttpPublishRule> {
        let mut rules = BTreeMap::new();
        for (prefix, id) in entries {
            rules.insert(
                id.to_string(),
                HttpPublishRule {
                    route_prefix: prefix.to_string(),
                    workspace: workspace_ref("shell"),
                    default_doc: None,
                    allow_dir_listing: false,
                    cache: "etag".to_string(),
                },
            );
        }
        rules
    }

    fn workspace_ref(name: &str) -> WorkspaceRef {
        WorkspaceRef {
            workspace: name.to_string(),
            version: None,
            path: None,
        }
    }
}
