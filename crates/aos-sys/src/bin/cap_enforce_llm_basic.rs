//! Cap enforcer for basic LLM generation (`sys/CapEnforceLlmBasic@1`).

#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use aos_sys::{BudgetMap, CapCheckInput, CapCheckOutput, CapDenyReason};
use aos_wasm_sdk::{PureError, PureModule, aos_pure};
use serde::de::DeserializeOwned;
use serde::Deserialize;
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
    tools: Option<Vec<String>>,
}

impl PureModule for CapEnforceLlmBasic {
    type Input = CapCheckInput;
    type Output = CapCheckOutput;

    fn run(&mut self, input: Self::Input) -> Result<Self::Output, PureError> {
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
            let tools = effect_params.tools.as_deref().unwrap_or(&[]);
            if !allowed.is_empty() && !tools.iter().all(|tool| allowed.iter().any(|t| t == tool)) {
                return Ok(deny("tool_not_allowed", "tool not allowed"));
            }
        }

        let mut reserve = BudgetMap::new();
        if effect_params.max_tokens > 0 {
            reserve.insert("tokens".into(), effect_params.max_tokens);
        }
        Ok(CapCheckOutput {
            constraints_ok: true,
            deny: None,
            reserve_estimate: reserve,
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
        reserve_estimate: BudgetMap::new(),
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
