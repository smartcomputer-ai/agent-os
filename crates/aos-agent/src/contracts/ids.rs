use alloc::string::String;
use aos_wasm_sdk::AirSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default, AirSchema)]
#[aos(schema = "aos.agent/SessionId@1", air_type = "uuid")]
pub struct SessionId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default, AirSchema)]
#[aos(schema = "aos.agent/RunId@1")]
pub struct RunId {
    pub session_id: SessionId,
    pub run_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default, AirSchema)]
#[aos(schema = "aos.agent/ToolBatchId@1")]
pub struct ToolBatchId {
    pub run_id: RunId,
    pub batch_seq: u64,
}
