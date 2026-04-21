use std::pin::Pin;

use fabric_protocol::{ErrorResponse, ExecEvent, HealthResponse};
use futures_core::Stream;
use futures_util::stream;
use reqwest::StatusCode;
use thiserror::Error;

pub type ExecEventClientStream =
    Pin<Box<dyn Stream<Item = Result<ExecEvent, FabricClientError>> + Send + 'static>>;

#[derive(Debug, Clone)]
pub(crate) struct FabricHttpClient {
    pub(crate) base_url: String,
    pub(crate) http: reqwest::Client,
    bearer_token: Option<String>,
}

impl FabricHttpClient {
    pub(crate) fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            http: reqwest::Client::new(),
            bearer_token: None,
        }
    }

    pub(crate) fn with_bearer_token(mut self, token: impl Into<String>) -> Self {
        self.bearer_token = Some(token.into());
        self
    }

    pub(crate) async fn health(&self) -> Result<HealthResponse, FabricClientError> {
        let response = self
            .http
            .get(format!("{}/healthz", self.base_url))
            .send()
            .await?;

        decode_json_response(response).await
    }

    pub(crate) fn apply_auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.bearer_token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }
}

#[derive(Debug, Error)]
pub enum FabricClientError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid payload: {0}")]
    InvalidPayload(String),
    #[error("server error {status} {code}: {message}")]
    Server {
        status: StatusCode,
        code: String,
        message: String,
    },
}

fn decode_exec_line(line: &[u8]) -> Result<ExecEvent, FabricClientError> {
    match serde_json::from_slice(line) {
        Ok(event) => Ok(event),
        Err(event_error) => match serde_json::from_slice::<ErrorResponse>(line) {
            Ok(error) => Err(FabricClientError::Server {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: error.code,
                message: error.message,
            }),
            Err(_) => Err(FabricClientError::Json(event_error)),
        },
    }
}

pub(crate) fn decode_exec_stream(response: reqwest::Response) -> ExecEventClientStream {
    Box::pin(stream::unfold(
        (response, Vec::<u8>::new(), false),
        |(mut response, mut pending, done)| async move {
            if done {
                return None;
            }

            loop {
                if let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
                    let line = pending.drain(..=newline).collect::<Vec<_>>();
                    if line.iter().all(|byte| byte.is_ascii_whitespace()) {
                        continue;
                    }
                    return Some((decode_exec_line(&line), (response, pending, false)));
                }

                match response.chunk().await {
                    Ok(Some(chunk)) => pending.extend_from_slice(&chunk),
                    Ok(None) if pending.is_empty() => return None,
                    Ok(None) => {
                        let line = std::mem::take(&mut pending);
                        return Some((decode_exec_line(&line), (response, pending, true)));
                    }
                    Err(error) => {
                        return Some((
                            Err(FabricClientError::Http(error)),
                            (response, pending, true),
                        ));
                    }
                }
            }
        },
    ))
}

pub(crate) async fn decode_json_response<T>(
    response: reqwest::Response,
) -> Result<T, FabricClientError>
where
    T: serde::de::DeserializeOwned,
{
    if !response.status().is_success() {
        return Err(decode_error_response(response).await);
    }

    Ok(response.json().await?)
}

pub(crate) async fn decode_error_response(response: reqwest::Response) -> FabricClientError {
    let status = response.status();
    match response.bytes().await {
        Ok(body) => match serde_json::from_slice::<ErrorResponse>(&body) {
            Ok(error) => FabricClientError::Server {
                status,
                code: error.code,
                message: error.message,
            },
            Err(_) => FabricClientError::Server {
                status,
                code: "http_error".to_owned(),
                message: String::from_utf8_lossy(&body).into_owned(),
            },
        },
        Err(error) => FabricClientError::Http(error),
    }
}
