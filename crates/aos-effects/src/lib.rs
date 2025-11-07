//! Effect intent and receipt types (skeleton)

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectIntent {
    pub kind: String,
    pub cap_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReceiptStatus {
    Ok,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub intent_kind: String,
    pub status: ReceiptStatus,
}
