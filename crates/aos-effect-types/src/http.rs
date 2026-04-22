use alloc::collections::BTreeMap;
use alloc::string::String;
use serde::{Deserialize, Serialize};

use crate::HashRef;

pub type HeaderMap = BTreeMap<String, String>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpRequestParams {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: HeaderMap,
    #[serde(default)]
    pub body_ref: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpRequestReceipt {
    pub status: i32,
    #[serde(default)]
    pub headers: HeaderMap,
    #[serde(default)]
    pub body_ref: Option<HashRef>,
    pub timings: RequestTimings,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestTimings {
    pub start_ns: u64,
    pub end_ns: u64,
}
