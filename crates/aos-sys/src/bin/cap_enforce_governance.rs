//! Cap enforcer for governance effects (`sys/CapEnforceGovernance@1`).

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

// Required for WASM binary entry point
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

aos_pure!(CapEnforceGovernance);

#[derive(Default)]
struct CapEnforceGovernance;

#[derive(Deserialize)]
struct GovernanceCapParams {
    ops: Option<Vec<String>>,
    def_kinds: Option<Vec<String>>,
    name_prefixes: Option<Vec<String>>,
    manifest_sections: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct GovProposeParams {
    summary: GovPatchSummary,
}

#[derive(Deserialize)]
struct GovPatchSummary {
    #[serde(default)]
    ops: Vec<String>,
    #[serde(default)]
    def_changes: Vec<GovDefChange>,
    #[serde(default)]
    manifest_sections: Vec<String>,
}

#[derive(Deserialize)]
struct GovDefChange {
    kind: String,
    name: String,
    #[serde(rename = "action")]
    _action: GovChangeAction,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum GovChangeAction {
    Added,
    Removed,
    Changed,
}

impl PureModule for CapEnforceGovernance {
    type Input = CapCheckInput;
    type Output = CapCheckOutput;

    fn run(
        &mut self,
        input: Self::Input,
        _ctx: Option<&PureContext>,
    ) -> Result<Self::Output, PureError> {
        if input.effect_kind != "governance.propose" {
            return Ok(CapCheckOutput {
                constraints_ok: true,
                deny: None,
            });
        }
        let cap_params: GovernanceCapParams = match decode_cbor(&input.cap_params) {
            Ok(value) => value,
            Err(err) => {
                return Ok(deny("cap_params_invalid", err.to_string()));
            }
        };
        let params: GovProposeParams = match decode_cbor(&input.effect_params) {
            Ok(value) => value,
            Err(err) => {
                return Ok(deny("effect_params_invalid", err.to_string()));
            }
        };

        if let Some(allowed) = non_empty_list(&cap_params.ops) {
            for op in &params.summary.ops {
                if !allowed.iter().any(|entry| entry == op) {
                    return Ok(deny(
                        "op_not_allowed",
                        format!("patch op '{op}' not allowed"),
                    ));
                }
            }
        }
        if let Some(allowed) = non_empty_list(&cap_params.def_kinds) {
            for change in &params.summary.def_changes {
                if !allowed.iter().any(|entry| entry == &change.kind) {
                    return Ok(deny(
                        "def_kind_not_allowed",
                        format!("def kind '{}' not allowed", change.kind),
                    ));
                }
            }
        }
        if let Some(prefixes) = non_empty_list(&cap_params.name_prefixes) {
            for change in &params.summary.def_changes {
                if !prefixes
                    .iter()
                    .any(|prefix| change.name.starts_with(prefix))
                {
                    return Ok(deny(
                        "name_prefix_not_allowed",
                        format!("def name '{}' not allowed", change.name),
                    ));
                }
            }
        }
        if let Some(allowed) = non_empty_list(&cap_params.manifest_sections) {
            for section in &params.summary.manifest_sections {
                if !allowed.iter().any(|entry| entry == section) {
                    return Ok(deny(
                        "manifest_section_not_allowed",
                        format!("manifest section '{section}' not allowed"),
                    ));
                }
            }
        }

        Ok(CapCheckOutput {
            constraints_ok: true,
            deny: None,
        })
    }
}

fn non_empty_list(list: &Option<Vec<String>>) -> Option<&Vec<String>> {
    match list {
        Some(list) if !list.is_empty() => Some(list),
        _ => None,
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

fn decode_cbor<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, PureError> {
    serde_cbor::from_slice(bytes).map_err(|_| PureError::new("decode_cbor_failed"))
}
