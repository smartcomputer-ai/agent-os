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
    #[serde(default)]
    pub dest_universe: Option<String>,
    pub dest_world: String,
    pub mode: PortalSendMode,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default, with = "crate::serde_helpers::bytes_opt")]
    pub value_cbor: Option<Vec<u8>>,
    #[serde(default)]
    pub inbox: Option<String>,
    #[serde(default, with = "crate::serde_helpers::bytes_opt")]
    pub payload_cbor: Option<Vec<u8>>,
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortalSendReceipt {
    pub status: String,
    pub message_id: String,
    pub dest_world: String,
    #[serde(default, with = "crate::serde_helpers::bytes_opt")]
    pub enqueued_seq: Option<Vec<u8>>,
}
