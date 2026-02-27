//! Error taxonomy and retry utilities.
//!
//! Implemented in P03.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use thiserror::Error;

/// Shared error message and cause payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
}

impl ErrorInfo {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cause: None,
        }
    }

    pub fn with_cause(message: impl Into<String>, cause: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cause: Some(cause.into()),
        }
    }
}

/// Provider error classification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    Authentication,
    AccessDenied,
    NotFound,
    InvalidRequest,
    RateLimit,
    Server,
    ContentFilter,
    ContextLength,
    QuotaExceeded,
    Other,
}

/// Base provider error with metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderError {
    pub info: ErrorInfo,
    pub provider: String,
    pub kind: ProviderErrorKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestTimeoutError {
    pub info: ErrorInfo,
    pub retryable: bool,
}

impl RequestTimeoutError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            info: ErrorInfo::new(message),
            retryable: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AbortError {
    pub info: ErrorInfo,
    pub retryable: bool,
}

impl AbortError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            info: ErrorInfo::new(message),
            retryable: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetworkError {
    pub info: ErrorInfo,
    pub retryable: bool,
}

impl NetworkError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            info: ErrorInfo::new(message),
            retryable: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StreamError {
    pub info: ErrorInfo,
    pub retryable: bool,
}

impl StreamError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            info: ErrorInfo::new(message),
            retryable: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InvalidToolCallError {
    pub info: ErrorInfo,
    pub retryable: bool,
}

impl InvalidToolCallError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            info: ErrorInfo::new(message),
            retryable: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoObjectGeneratedError {
    pub info: ErrorInfo,
    pub retryable: bool,
}

impl NoObjectGeneratedError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            info: ErrorInfo::new(message),
            retryable: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfigurationError {
    pub info: ErrorInfo,
    pub retryable: bool,
}

impl ConfigurationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            info: ErrorInfo::new(message),
            retryable: false,
        }
    }
}

/// Unified SDK error type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Error)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum SDKError {
    #[error("{0}")]
    Provider(ProviderError),
    #[error("{0}")]
    RequestTimeout(RequestTimeoutError),
    #[error("{0}")]
    Abort(AbortError),
    #[error("{0}")]
    Network(NetworkError),
    #[error("{0}")]
    Stream(StreamError),
    #[error("{0}")]
    InvalidToolCall(InvalidToolCallError),
    #[error("{0}")]
    NoObjectGenerated(NoObjectGeneratedError),
    #[error("{0}")]
    Configuration(ConfigurationError),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.info.message)
    }
}

impl std::fmt::Display for RequestTimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.info.message)
    }
}

impl std::fmt::Display for AbortError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.info.message)
    }
}

impl std::fmt::Display for NetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.info.message)
    }
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.info.message)
    }
}

impl std::fmt::Display for InvalidToolCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.info.message)
    }
}

impl std::fmt::Display for NoObjectGeneratedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.info.message)
    }
}

impl std::fmt::Display for ConfigurationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.info.message)
    }
}

impl SDKError {
    pub fn message(&self) -> &str {
        match self {
            SDKError::Provider(err) => &err.info.message,
            SDKError::RequestTimeout(err) => &err.info.message,
            SDKError::Abort(err) => &err.info.message,
            SDKError::Network(err) => &err.info.message,
            SDKError::Stream(err) => &err.info.message,
            SDKError::InvalidToolCall(err) => &err.info.message,
            SDKError::NoObjectGenerated(err) => &err.info.message,
            SDKError::Configuration(err) => &err.info.message,
        }
    }

    pub fn retryable(&self) -> bool {
        match self {
            SDKError::Provider(err) => err.retryable,
            SDKError::RequestTimeout(err) => err.retryable,
            SDKError::Abort(err) => err.retryable,
            SDKError::Network(err) => err.retryable,
            SDKError::Stream(err) => err.retryable,
            SDKError::InvalidToolCall(err) => err.retryable,
            SDKError::NoObjectGenerated(err) => err.retryable,
            SDKError::Configuration(err) => err.retryable,
        }
    }
}

impl ProviderError {
    pub fn new(
        provider: impl Into<String>,
        kind: ProviderErrorKind,
        message: impl Into<String>,
    ) -> Self {
        let retryable = default_retryable_for_kind(&kind);
        Self {
            info: ErrorInfo::new(message),
            provider: provider.into(),
            kind,
            status_code: None,
            error_code: None,
            retryable,
            retry_after: None,
            raw: None,
        }
    }
}

/// HTTP status classification result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpErrorClassification {
    Provider(ProviderErrorKind, bool),
    RequestTimeout(bool),
}

/// Map HTTP status codes to error classification.
pub fn map_http_status(status: u16) -> Option<HttpErrorClassification> {
    match status {
        400 => Some(HttpErrorClassification::Provider(
            ProviderErrorKind::InvalidRequest,
            false,
        )),
        401 => Some(HttpErrorClassification::Provider(
            ProviderErrorKind::Authentication,
            false,
        )),
        403 => Some(HttpErrorClassification::Provider(
            ProviderErrorKind::AccessDenied,
            false,
        )),
        404 => Some(HttpErrorClassification::Provider(
            ProviderErrorKind::NotFound,
            false,
        )),
        408 => Some(HttpErrorClassification::RequestTimeout(true)),
        413 => Some(HttpErrorClassification::Provider(
            ProviderErrorKind::ContextLength,
            false,
        )),
        422 => Some(HttpErrorClassification::Provider(
            ProviderErrorKind::InvalidRequest,
            false,
        )),
        429 => Some(HttpErrorClassification::Provider(
            ProviderErrorKind::RateLimit,
            true,
        )),
        500 | 502 | 503 | 504 => Some(HttpErrorClassification::Provider(
            ProviderErrorKind::Server,
            true,
        )),
        _ => None,
    }
}

/// Map gRPC status codes to error classification.
pub fn map_grpc_status(code: &str) -> Option<HttpErrorClassification> {
    match code.to_ascii_uppercase().as_str() {
        "NOT_FOUND" => Some(HttpErrorClassification::Provider(
            ProviderErrorKind::NotFound,
            false,
        )),
        "INVALID_ARGUMENT" => Some(HttpErrorClassification::Provider(
            ProviderErrorKind::InvalidRequest,
            false,
        )),
        "UNAUTHENTICATED" => Some(HttpErrorClassification::Provider(
            ProviderErrorKind::Authentication,
            false,
        )),
        "PERMISSION_DENIED" => Some(HttpErrorClassification::Provider(
            ProviderErrorKind::AccessDenied,
            false,
        )),
        "RESOURCE_EXHAUSTED" => Some(HttpErrorClassification::Provider(
            ProviderErrorKind::RateLimit,
            true,
        )),
        "UNAVAILABLE" => Some(HttpErrorClassification::Provider(
            ProviderErrorKind::Server,
            true,
        )),
        "DEADLINE_EXCEEDED" => Some(HttpErrorClassification::RequestTimeout(true)),
        "INTERNAL" => Some(HttpErrorClassification::Provider(
            ProviderErrorKind::Server,
            true,
        )),
        _ => None,
    }
}

/// Classify error kind from message content when status codes are ambiguous.
pub fn classify_message(message: &str) -> Option<ProviderErrorKind> {
    let lower = message.to_ascii_lowercase();
    if lower.contains("not found") || lower.contains("does not exist") {
        return Some(ProviderErrorKind::NotFound);
    }
    if lower.contains("unauthorized") || lower.contains("invalid key") {
        return Some(ProviderErrorKind::Authentication);
    }
    if lower.contains("context length") || lower.contains("too many tokens") {
        return Some(ProviderErrorKind::ContextLength);
    }
    if lower.contains("content filter") || lower.contains("safety") {
        return Some(ProviderErrorKind::ContentFilter);
    }
    None
}

pub fn default_retryable_for_kind(kind: &ProviderErrorKind) -> bool {
    matches!(
        kind,
        ProviderErrorKind::RateLimit | ProviderErrorKind::Server | ProviderErrorKind::Other
    )
}

/// Retry policy configuration.
#[derive(Clone)]
pub struct RetryPolicy {
    pub max_retries: usize,
    pub base_delay: f64,
    pub max_delay: f64,
    pub backoff_multiplier: f64,
    pub jitter: bool,
    pub on_retry: Option<Arc<dyn Fn(&SDKError, usize, f64) + Send + Sync>>,
}

impl std::fmt::Debug for RetryPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetryPolicy")
            .field("max_retries", &self.max_retries)
            .field("base_delay", &self.base_delay)
            .field("max_delay", &self.max_delay)
            .field("backoff_multiplier", &self.backoff_multiplier)
            .field("jitter", &self.jitter)
            .field("on_retry", &self.on_retry.is_some())
            .finish()
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            base_delay: 1.0,
            max_delay: 60.0,
            backoff_multiplier: 2.0,
            jitter: true,
            on_retry: None,
        }
    }
}

/// Compute the delay for a retry attempt. Returns None when no retry should occur.
pub fn compute_backoff_delay(
    policy: &RetryPolicy,
    attempt: usize,
    retry_after: Option<f64>,
) -> Option<f64> {
    if let Some(retry_after) = retry_after {
        if retry_after <= policy.max_delay {
            return Some(retry_after);
        }
        return None;
    }

    let raw = policy.base_delay * policy.backoff_multiplier.powi(attempt as i32);
    let capped = raw.min(policy.max_delay);
    if policy.jitter {
        Some(capped * jitter_factor(attempt))
    } else {
        Some(capped)
    }
}

/// Retry an async operation according to the provided retry policy.
///
/// Retries are attempted only for retryable errors. When a provider error
/// includes `retry_after`, it is respected by `compute_backoff_delay`.
pub async fn retry_async<T, Op, Fut>(policy: &RetryPolicy, mut operation: Op) -> Result<T, SDKError>
where
    Op: FnMut() -> Fut,
    Fut: Future<Output = Result<T, SDKError>>,
{
    let mut attempt = 0usize;

    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                if !error.retryable() || attempt >= policy.max_retries {
                    return Err(error);
                }

                let retry_after = match &error {
                    SDKError::Provider(provider_error) => provider_error.retry_after,
                    _ => None,
                };
                let Some(delay) = compute_backoff_delay(policy, attempt, retry_after) else {
                    return Err(error);
                };

                if let Some(on_retry) = &policy.on_retry {
                    on_retry(&error, attempt, delay);
                }

                tokio::time::sleep(std::time::Duration::from_secs_f64(delay)).await;
                attempt += 1;
            }
        }
    }
}

fn jitter_factor(attempt: usize) -> f64 {
    // Deterministic +/-50% jitter derived from attempt.
    let mut x = (attempt as u64).wrapping_add(0x9e3779b97f4a7c15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    x ^= x >> 31;
    let normalized = (x % 10_000) as f64 / 10_000.0; // [0,1)
    0.5 + normalized
}

/// Timeout configuration for high-level operations.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimeoutConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_step: Option<f64>,
}

/// Adapter-level timeouts for HTTP operations.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdapterTimeout {
    pub connect: f64,
    pub request: f64,
    pub stream_read: f64,
}

impl Default for AdapterTimeout {
    fn default() -> Self {
        Self {
            connect: 10.0,
            request: 120.0,
            stream_read: 30.0,
        }
    }
}

/// Abort signal shared between callers and async operations.
#[derive(Clone, Debug)]
pub struct AbortSignal {
    flag: Arc<AtomicBool>,
}

impl AbortSignal {
    pub fn is_aborted(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    pub fn abort(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }
}

/// Abort controller that owns the underlying signal.
#[derive(Clone, Debug)]
pub struct AbortController {
    signal: AbortSignal,
}

impl AbortController {
    pub fn new() -> Self {
        Self {
            signal: AbortSignal {
                flag: Arc::new(AtomicBool::new(false)),
            },
        }
    }

    pub fn signal(&self) -> AbortSignal {
        self.signal.clone()
    }

    pub fn abort(&self) {
        self.signal.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_http_status_matches_spec() {
        assert_eq!(
            map_http_status(401),
            Some(HttpErrorClassification::Provider(
                ProviderErrorKind::Authentication,
                false
            ))
        );
        assert_eq!(
            map_http_status(429),
            Some(HttpErrorClassification::Provider(
                ProviderErrorKind::RateLimit,
                true
            ))
        );
        assert_eq!(
            map_http_status(504),
            Some(HttpErrorClassification::Provider(
                ProviderErrorKind::Server,
                true
            ))
        );
        assert_eq!(
            map_http_status(408),
            Some(HttpErrorClassification::RequestTimeout(true))
        );
        assert_eq!(map_http_status(499), None);
    }

    #[test]
    fn classify_message_detects_signals() {
        assert_eq!(
            classify_message("Model not found"),
            Some(ProviderErrorKind::NotFound)
        );
        assert_eq!(
            classify_message("Context length exceeded"),
            Some(ProviderErrorKind::ContextLength)
        );
        assert_eq!(
            classify_message("Content filter triggered"),
            Some(ProviderErrorKind::ContentFilter)
        );
        assert_eq!(classify_message("Unknown"), None);
    }

    #[test]
    fn backoff_without_jitter_is_deterministic() {
        let policy = RetryPolicy {
            jitter: false,
            ..RetryPolicy::default()
        };
        let d0 = compute_backoff_delay(&policy, 0, None).unwrap();
        let d1 = compute_backoff_delay(&policy, 1, None).unwrap();
        let d2 = compute_backoff_delay(&policy, 2, None).unwrap();
        assert_eq!(d0, 1.0);
        assert_eq!(d1, 2.0);
        assert_eq!(d2, 4.0);
    }

    #[test]
    fn backoff_with_jitter_stays_in_range() {
        let policy = RetryPolicy::default();
        let delay = compute_backoff_delay(&policy, 1, None).unwrap();
        assert!(delay >= 1.0 && delay <= 3.0);
    }

    #[test]
    fn retry_after_overrides_when_within_max() {
        let policy = RetryPolicy::default();
        let delay = compute_backoff_delay(&policy, 1, Some(10.0)).unwrap();
        assert_eq!(delay, 10.0);
    }

    #[test]
    fn retry_after_exceeding_max_disables_retry() {
        let policy = RetryPolicy::default();
        let delay = compute_backoff_delay(&policy, 1, Some(120.0));
        assert!(delay.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retry_async_retries_retryable_error_and_succeeds() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let attempts = AtomicUsize::new(0);
        let policy = RetryPolicy {
            max_retries: 2,
            jitter: false,
            ..RetryPolicy::default()
        };

        let result = retry_async(&policy, || {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                if attempt == 0 {
                    Err(SDKError::Provider(ProviderError {
                        info: ErrorInfo::new("rate limited"),
                        provider: "openai".to_string(),
                        kind: ProviderErrorKind::RateLimit,
                        status_code: Some(429),
                        error_code: None,
                        retryable: true,
                        retry_after: Some(0.0),
                        raw: None,
                    }))
                } else {
                    Ok("ok")
                }
            }
        })
        .await;

        assert_eq!(result.expect("result"), "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
