use alloc::string::String;
use aos_wasm_sdk::AirSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/HostCommand@1")]
pub struct HostCommand {
    pub command_id: String,
    #[aos(air_type = "time")]
    pub issued_at: u64,
    #[aos(
        type_json = r#"{"variant":{"Steer":{"record":{"text":{"text":{}}}},"FollowUp":{"record":{"text":{"text":{}}}},"Pause":{"unit":{}},"Resume":{"unit":{}},"Cancel":{"record":{"reason":{"option":{"text":{}}}}},"Noop":{"unit":{}}}}"#
    )]
    pub command: HostCommandKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "$tag", content = "$value")]
pub enum HostCommandKind {
    Steer {
        text: String,
    },
    FollowUp {
        text: String,
    },
    Pause,
    Resume,
    Cancel {
        reason: Option<String>,
    },
    #[default]
    Noop,
}
