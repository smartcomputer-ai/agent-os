//! Cap enforcer for basic LLM generation (`sys/CapEnforceLlmBasic@1`).

#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use aos_sys::{CapCheckInput, CapCheckOutput, CapDenyReason};
use aos_wasm_abi::PureContext;
use aos_wasm_sdk::{PureError, PureModule, aos_pure};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_cbor::Value as CborValue;

// Required for WASM binary entry point
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

aos_pure!(CapEnforceLlmBasic);

#[derive(Default)]
struct CapEnforceLlmBasic;

#[derive(Deserialize)]
struct LlmCapParams {
    providers: Option<Vec<String>>,
    models: Option<Vec<String>>,
    max_tokens: Option<u64>,
    tools_allow: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct LlmGenerateParams {
    provider: String,
    model: String,
    max_tokens: u64,
    #[serde(default)]
    tool_choice: Option<LlmToolChoice>,
}

#[derive(Deserialize)]
#[serde(tag = "$tag", content = "$value")]
enum LlmToolChoice {
    Auto,
    #[serde(rename = "None")]
    NoneChoice,
    Required,
    Tool { name: String },
}

impl PureModule for CapEnforceLlmBasic {
    type Input = CapCheckInput;
    type Output = CapCheckOutput;

    fn run(
        &mut self,
        input: Self::Input,
        _ctx: Option<&PureContext>,
    ) -> Result<Self::Output, PureError> {
        if input.effect_kind != "llm.generate" {
            return Ok(deny(
                "effect_kind_mismatch",
                format!(
                    "enforcer 'sys/CapEnforceLlmBasic@1' cannot handle '{}'",
                    input.effect_kind
                ),
            ));
        }
        let cap_params: LlmCapParams = match decode_cbor(&input.cap_params) {
            Ok(value) => value,
            Err(err) => {
                return Ok(deny("cap_params_invalid", err.to_string()));
            }
        };
        let effect_params: LlmGenerateParams = match decode_cbor(&input.effect_params) {
            Ok(value) => value,
            Err(err) => {
                return Ok(deny("effect_params_invalid", err.to_string()));
            }
        };

        if !allowlist_contains(&cap_params.providers, &effect_params.provider, |v| {
            v.to_string()
        }) {
            return Ok(deny(
                "provider_not_allowed",
                format!("provider '{}' not allowed", effect_params.provider),
            ));
        }
        if !allowlist_contains(&cap_params.models, &effect_params.model, |v| v.to_string()) {
            return Ok(deny(
                "model_not_allowed",
                format!("model '{}' not allowed", effect_params.model),
            ));
        }
        if let Some(limit) = cap_params.max_tokens {
            if effect_params.max_tokens > limit {
                return Ok(deny(
                    "max_tokens_exceeded",
                    format!(
                        "max_tokens {} exceeds cap {limit}",
                        effect_params.max_tokens
                    ),
                ));
            }
        }
        if let Some(allowed) = &cap_params.tools_allow {
            if !allowed.is_empty() {
                if let Some(choice) = &effect_params.tool_choice {
                    if let LlmToolChoice::Tool { name } = choice {
                        if !allowed.iter().any(|t| t == name) {
                            return Ok(deny("tool_not_allowed", "tool not allowed"));
                        }
                    }
                }
            }
        }

        Ok(CapCheckOutput {
            constraints_ok: true,
            deny: None,
        })
    }
}

fn deny(code: &str, message: impl Into<String>) -> CapCheckOutput {
    CapCheckOutput {
        constraints_ok: false,
        deny: Some(CapDenyReason {
            code: code.into(),
            message: message.into(),
        }),
    }
}

fn decode_cbor<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, serde_cbor::Error> {
    const CBOR_SELF_DESCRIBE_TAG: u64 = 55799;
    let value: CborValue = serde_cbor::from_slice(bytes)?;
    let value = match value {
        CborValue::Tag(tag, inner) if tag == CBOR_SELF_DESCRIBE_TAG => *inner,
        other => other,
    };
    serde_cbor::value::from_value(value)
}

fn allowlist_contains(
    list: &Option<Vec<String>>,
    value: &str,
    normalize: impl Fn(&str) -> String,
) -> bool {
    let Some(list) = list else {
        return true;
    };
    if list.is_empty() {
        return true;
    }
    let value = normalize(value);
    list.iter().any(|entry| normalize(entry) == value)
}
