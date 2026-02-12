//! HTTP client helpers for provider adapters.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method, RequestBuilder};

use crate::errors::AdapterTimeout;

/// Simple HTTP client wrapper with consistent defaults.
#[derive(Clone, Debug)]
pub struct HttpClient {
    client: Client,
    default_headers: HeaderMap,
    timeout: AdapterTimeout,
}

impl HttpClient {
    pub fn new(
        timeout: AdapterTimeout,
        default_headers: HeaderMap,
    ) -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs_f64(timeout.connect))
            .timeout(Duration::from_secs_f64(timeout.request))
            .build()?;

        Ok(Self {
            client,
            default_headers,
            timeout,
        })
    }

    pub fn timeout(&self) -> AdapterTimeout {
        self.timeout
    }

    pub fn request(&self, method: Method, url: &str) -> RequestBuilder {
        let mut builder = self.client.request(method, url);
        if !self.default_headers.is_empty() {
            builder = builder.headers(self.default_headers.clone());
        }
        builder
    }

    pub fn with_header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.default_headers.insert(name, value);
        self
    }
}
