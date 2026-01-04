//! Cap enforcer for workspace effects (`sys/CapEnforceWorkspace@1`).

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

aos_pure!(CapEnforceWorkspace);

#[derive(Default)]
struct CapEnforceWorkspace;

#[derive(Deserialize)]
struct WorkspaceCapParams {
    workspaces: Option<Vec<String>>,
    path_prefixes: Option<Vec<String>>,
    ops: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct WorkspaceResolveParams {
    workspace: String,
}

#[derive(Deserialize)]
struct WorkspaceListParams {
    path: Option<String>,
}

#[derive(Deserialize)]
struct WorkspaceReadRefParams {
    path: String,
}

#[derive(Deserialize)]
struct WorkspaceReadBytesParams {
    path: String,
}

#[derive(Deserialize)]
struct WorkspaceWriteBytesParams {
    path: String,
}

#[derive(Deserialize)]
struct WorkspaceRemoveParams {
    path: String,
}

#[derive(Deserialize)]
struct WorkspaceDiffParams {
    prefix: Option<String>,
}

impl PureModule for CapEnforceWorkspace {
    type Input = CapCheckInput;
    type Output = CapCheckOutput;

    fn run(
        &mut self,
        input: Self::Input,
        _ctx: Option<&PureContext>,
    ) -> Result<Self::Output, PureError> {
        let Some(op) = op_for_kind(&input.effect_kind) else {
            return Ok(deny(
                "effect_kind_mismatch",
                format!(
                    "enforcer 'sys/CapEnforceWorkspace@1' cannot handle '{}'",
                    input.effect_kind
                ),
            ));
        };
        let cap_params: WorkspaceCapParams = match decode_cbor(&input.cap_params) {
            Ok(value) => value,
            Err(err) => {
                return Ok(deny("cap_params_invalid", err.to_string()));
            }
        };
        if !allowlist_contains(&cap_params.ops, op, |v| v.to_lowercase()) {
            return Ok(deny(
                "op_not_allowed",
                format!("op '{op}' not allowed"),
            ));
        }

        match input.effect_kind.as_str() {
            "workspace.resolve" => {
                let params: WorkspaceResolveParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => {
                        return Ok(deny("effect_params_invalid", err.to_string()));
                    }
                };
                if !allowlist_contains(&cap_params.workspaces, &params.workspace, |v| {
                    v.to_string()
                }) {
                    return Ok(deny(
                        "workspace_not_allowed",
                        format!("workspace '{}' not allowed", params.workspace),
                    ));
                }
            }
            "workspace.list" => {
                let params: WorkspaceListParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => {
                        return Ok(deny("effect_params_invalid", err.to_string()));
                    }
                };
                let path = params.path.unwrap_or_default();
                if !path_allowed(&cap_params.path_prefixes, &path) {
                    return Ok(deny(
                        "path_not_allowed",
                        format!("path '{}' not allowed", path),
                    ));
                }
            }
            "workspace.read_ref" => {
                let params: WorkspaceReadRefParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => {
                        return Ok(deny("effect_params_invalid", err.to_string()));
                    }
                };
                if !path_allowed(&cap_params.path_prefixes, &params.path) {
                    return Ok(deny(
                        "path_not_allowed",
                        format!("path '{}' not allowed", params.path),
                    ));
                }
            }
            "workspace.read_bytes" => {
                let params: WorkspaceReadBytesParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => {
                        return Ok(deny("effect_params_invalid", err.to_string()));
                    }
                };
                if !path_allowed(&cap_params.path_prefixes, &params.path) {
                    return Ok(deny(
                        "path_not_allowed",
                        format!("path '{}' not allowed", params.path),
                    ));
                }
            }
            "workspace.write_bytes" => {
                let params: WorkspaceWriteBytesParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => {
                        return Ok(deny("effect_params_invalid", err.to_string()));
                    }
                };
                if !path_allowed(&cap_params.path_prefixes, &params.path) {
                    return Ok(deny(
                        "path_not_allowed",
                        format!("path '{}' not allowed", params.path),
                    ));
                }
            }
            "workspace.remove" => {
                let params: WorkspaceRemoveParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => {
                        return Ok(deny("effect_params_invalid", err.to_string()));
                    }
                };
                if !path_allowed(&cap_params.path_prefixes, &params.path) {
                    return Ok(deny(
                        "path_not_allowed",
                        format!("path '{}' not allowed", params.path),
                    ));
                }
            }
            "workspace.diff" => {
                let params: WorkspaceDiffParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => {
                        return Ok(deny("effect_params_invalid", err.to_string()));
                    }
                };
                let path = params.prefix.unwrap_or_default();
                if !path_allowed(&cap_params.path_prefixes, &path) {
                    return Ok(deny(
                        "path_not_allowed",
                        format!("path '{}' not allowed", path),
                    ));
                }
            }
            _ => {
                return Ok(deny(
                    "effect_kind_mismatch",
                    format!(
                        "enforcer 'sys/CapEnforceWorkspace@1' cannot handle '{}'",
                        input.effect_kind
                    ),
                ));
            }
        }

        Ok(CapCheckOutput {
            constraints_ok: true,
            deny: None,
        })
    }
}

fn op_for_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "workspace.resolve" => Some("resolve"),
        "workspace.list" => Some("list"),
        "workspace.read_ref" | "workspace.read_bytes" => Some("read"),
        "workspace.write_bytes" | "workspace.remove" => Some("write"),
        "workspace.diff" => Some("diff"),
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

fn path_allowed(prefixes: &Option<Vec<String>>, path: &str) -> bool {
    let Some(prefixes) = prefixes else {
        return true;
    };
    if prefixes.is_empty() {
        return true;
    }
    for prefix in prefixes {
        if prefix.is_empty() {
            return true;
        }
        if path == prefix {
            return true;
        }
        if path.starts_with(prefix) {
            if path.len() == prefix.len() {
                return true;
            }
            if path.as_bytes().get(prefix.len()) == Some(&b'/') {
                return true;
            }
        }
    }
    false
}
