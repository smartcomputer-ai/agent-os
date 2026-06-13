use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

use crate::config::ProfileKind;

#[derive(Debug, Clone)]
pub struct ApiTarget {
    pub kind: ProfileKind,
    pub api: String,
    pub headers: BTreeMap<String, String>,
    pub token: Option<String>,
    pub verbose: bool,
    pub world: Option<String>,
}

#[derive(Clone)]
pub struct ApiClient {
    base_url: String,
    client: reqwest::Client,
    verbose: bool,
}

impl ApiClient {
    pub fn new(target: &ApiTarget) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(token) = &target.token {
            let value = HeaderValue::from_str(&format!("Bearer {token}"))
                .context("build authorization header")?;
            headers.insert(AUTHORIZATION, value);
        }
        for (name, value) in &target.headers {
            headers.insert(
                HeaderName::from_bytes(name.as_bytes())
                    .with_context(|| format!("invalid header name '{name}'"))?,
                HeaderValue::from_str(value)
                    .with_context(|| format!("invalid header value for '{name}'"))?,
            );
        }
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(300))
            .build()
            .context("build reqwest client")?;
        Ok(Self {
            base_url: target.api.trim_end_matches('/').to_string(),
            client,
            verbose: target.verbose,
        })
    }

    pub async fn get_json(&self, path: &str, query: &[(&str, Option<String>)]) -> Result<Value> {
        let mut request = self.client.get(self.url(path));
        for (key, value) in query {
            if let Some(value) = value {
                request = request.query(&[(key, value)]);
            }
        }
        self.send_json("GET", path, request, None).await
    }

    pub async fn get_stream(
        &self,
        path: &str,
        query: &[(&str, Option<String>)],
    ) -> Result<reqwest::Response> {
        let mut request = self.client.get(self.url(path));
        for (key, value) in query {
            if let Some(value) = value {
                request = request.query(&[(key, value)]);
            }
        }
        self.log_request("GET", path, None);
        let response = request
            .send()
            .await
            .with_context(|| format!("send GET {}", self.url(path)))?;
        self.log_response("GET", path, response.status());
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("{}", format_http_error("GET", path, status, &body)));
        }
        Ok(response)
    }

    pub async fn delete_json(&self, path: &str, query: &[(&str, Option<String>)]) -> Result<Value> {
        let mut request = self.client.delete(self.url(path));
        for (key, value) in query {
            if let Some(value) = value {
                request = request.query(&[(key, value)]);
            }
        }
        self.send_json("DELETE", path, request, None).await
    }

    pub async fn post_json(&self, path: &str, body: &Value) -> Result<Value> {
        let request = self.client.post(self.url(path)).json(body);
        self.send_json("POST", path, request, None).await
    }

    pub async fn put_json(&self, path: &str, body: &Value) -> Result<Value> {
        let request = self.client.put(self.url(path)).json(body);
        self.send_json("PUT", path, request, None).await
    }

    pub async fn put_bytes(&self, path: &str, body: Vec<u8>) -> Result<Value> {
        let body_len = body.len();
        let request = self
            .client
            .put(self.url(path))
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(body);
        self.send_json("PUT", path, request, Some(body_len)).await
    }

    pub async fn get_bytes(&self, path: &str, query: &[(&str, Option<String>)]) -> Result<Vec<u8>> {
        let mut request = self.client.get(self.url(path));
        for (key, value) in query {
            if let Some(value) = value {
                request = request.query(&[(key, value)]);
            }
        }
        self.log_request("GET", path, None);
        let response = request
            .send()
            .await
            .with_context(|| format!("send GET {}", self.url(path)))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("{}", format_http_error("GET", path, status, &body)));
        }
        self.log_response("GET", path, response.status());
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .with_context(|| format!("read response bytes from GET {}", self.url(path)))
    }

    pub async fn head_exists(&self, path: &str) -> Result<bool> {
        self.log_request("HEAD", path, None);
        let response = self
            .client
            .head(self.url(path))
            .send()
            .await
            .with_context(|| format!("send HEAD {}", self.url(path)))?;
        self.log_response("HEAD", path, response.status());
        Ok(response.status().is_success())
    }

    pub fn log(&self, message: impl AsRef<str>) {
        if self.verbose {
            eprintln!("verbose: {}", message.as_ref());
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    async fn send_json(
        &self,
        method: &str,
        path: &str,
        request: reqwest::RequestBuilder,
        body_len: Option<usize>,
    ) -> Result<Value> {
        self.log_request(method, path, body_len);
        let response = request
            .send()
            .await
            .with_context(|| format!("send {} {}", method, self.url(path)))?;
        let status = response.status();
        self.log_response(method, path, status);
        let body = response.text().await.context("read response body")?;
        if !status.is_success() {
            return Err(anyhow!(
                "{}",
                format_http_error(method, path, status, &body)
            ));
        }
        if body.is_empty() {
            Ok(Value::Null)
        } else {
            serde_json::from_str(&body).with_context(|| format!("decode json response: {body}"))
        }
    }

    fn log_request(&self, method: &str, path: &str, body_len: Option<usize>) {
        if !self.verbose {
            return;
        }
        match body_len {
            Some(body_len) => eprintln!(
                "verbose: {} {} ({} bytes)",
                method,
                self.url(path),
                body_len
            ),
            None => eprintln!("verbose: {} {}", method, self.url(path)),
        }
    }

    fn log_response(&self, method: &str, path: &str, status: reqwest::StatusCode) {
        if self.verbose {
            eprintln!("verbose: {} {} -> {}", method, self.url(path), status);
        }
    }
}

fn format_http_error(method: &str, path: &str, status: reqwest::StatusCode, body: &str) -> String {
    let trimmed = body.trim();
    let base = if trimmed.is_empty() {
        format!("request failed: {method} {path} -> {status}")
    } else {
        format!("request failed: {method} {path} -> {status}: {trimmed}")
    };
    if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
        format!(
            "{base}. The server rejected the request body as too large; this usually means a CAS blob upload exceeded the current HTTP body limit."
        )
    } else {
        base
    }
}
