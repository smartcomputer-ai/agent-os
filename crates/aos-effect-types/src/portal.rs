use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PortalSendMode {
    TypedEvent,
    Inbox,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortalSendParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest_universe: Option<String>,
    pub dest_world: String,
    pub mode: PortalSendMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_helpers::bytes_opt"
    )]
    pub value_cbor: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbox: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_helpers::bytes_opt"
    )]
    pub payload_cbor: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortalSendReceipt {
    pub status: String,
    pub message_id: String,
    pub dest_world: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_helpers::bytes_opt"
    )]
    pub enqueued_seq: Option<Vec<u8>>,
}
