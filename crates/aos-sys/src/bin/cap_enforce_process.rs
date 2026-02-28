//! Cap enforcer for process effects (`sys/CapEnforceProcess@1`).

#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::collections::BTreeMap;
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

aos_pure!(CapEnforceProcess);

#[derive(Default)]
struct CapEnforceProcess;

#[derive(Deserialize)]
struct ProcessCapParams {
    allowed_targets: Option<Vec<String>>,
    network_modes: Option<Vec<String>>,
    local_mount_roots: Option<Vec<String>>,
    local_guest_path_prefixes: Option<Vec<String>>,
    local_mount_modes: Option<Vec<String>>,
    workdir_prefixes: Option<Vec<String>>,
    env_allow: Option<Vec<String>>,
    max_mounts: Option<u64>,
    max_env_vars: Option<u64>,
    max_session_ttl_ns: Option<u64>,
    max_exec_timeout_ns: Option<u64>,
    max_argv_len: Option<u64>,
    max_arg_length: Option<u64>,
    allowed_output_modes: Option<Vec<String>>,
    allowed_signals: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessTarget {
    local: Option<ProcessLocalTarget>,
}

#[derive(Deserialize)]
struct ProcessLocalTarget {
    mounts: Option<Vec<ProcessMount>>,
    workdir: Option<String>,
    env: Option<BTreeMap<String, String>>,
    network_mode: String,
}

#[derive(Deserialize)]
struct ProcessMount {
    host_path: String,
    guest_path: String,
    mode: String,
}

#[derive(Deserialize)]
struct ProcessSessionOpenParams {
    target: ProcessTarget,
    session_ttl_ns: Option<u64>,
}

#[derive(Deserialize)]
struct ProcessExecParams {
    argv: Vec<String>,
    cwd: Option<String>,
    timeout_ns: Option<u64>,
    env_patch: Option<BTreeMap<String, String>>,
    output_mode: Option<String>,
}

#[derive(Deserialize)]
struct ProcessSignalParams {
    signal: String,
}

impl PureModule for CapEnforceProcess {
    type Input = CapCheckInput;
    type Output = CapCheckOutput;

    fn run(
        &mut self,
        input: Self::Input,
        _ctx: Option<&PureContext>,
    ) -> Result<Self::Output, PureError> {
        let cap_params: ProcessCapParams = match decode_cbor(&input.cap_params) {
            Ok(value) => value,
            Err(err) => {
                return Ok(deny("cap_params_invalid", err.to_string()));
            }
        };

        match input.effect_kind.as_str() {
            "process.session.open" => {
                let params: ProcessSessionOpenParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => {
                        return Ok(deny("effect_params_invalid", err.to_string()));
                    }
                };
                validate_open(&cap_params, params)
            }
            "process.exec" => {
                let params: ProcessExecParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => {
                        return Ok(deny("effect_params_invalid", err.to_string()));
                    }
                };
                validate_exec(&cap_params, params)
            }
            "process.session.signal" => {
                let params: ProcessSignalParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => {
                        return Ok(deny("effect_params_invalid", err.to_string()));
                    }
                };
                validate_signal(&cap_params, params)
            }
            other => Ok(deny(
                "effect_kind_mismatch",
                format!(
                    "enforcer 'sys/CapEnforceProcess@1' cannot handle '{}'",
                    other
                ),
            )),
        }
    }
}

fn validate_open(
    cap: &ProcessCapParams,
    params: ProcessSessionOpenParams,
) -> Result<CapCheckOutput, PureError> {
    if !allowlist_contains(&cap.allowed_targets, "local", |v| v.to_ascii_lowercase()) {
        return Ok(deny(
            "target_not_allowed",
            "target 'local' not allowed by capability",
        ));
    }

    let Some(local) = params.target.local else {
        return Ok(deny("target_invalid", "missing target.local"));
    };

    if !allowlist_contains(&cap.network_modes, &local.network_mode, |v| {
        v.to_ascii_lowercase()
    }) {
        return Ok(deny(
            "network_mode_not_allowed",
            format!("network_mode '{}' not allowed", local.network_mode),
        ));
    }

    if let Some(limit) = cap.max_mounts {
        let count = local.mounts.as_ref().map_or(0, |m| m.len()) as u64;
        if count > limit {
            return Ok(deny(
                "mount_limit_exceeded",
                format!("mount count {count} exceeds max_mounts {limit}"),
            ));
        }
    }

    if let Some(mounts) = local.mounts.as_ref() {
        for mount in mounts {
            if !allowlist_contains(&cap.local_mount_modes, &mount.mode, |v| {
                v.to_ascii_lowercase()
            }) {
                return Ok(deny(
                    "mount_mode_not_allowed",
                    format!("mount mode '{}' not allowed", mount.mode),
                ));
            }
            if !path_allowed(&cap.local_mount_roots, &mount.host_path) {
                return Ok(deny(
                    "mount_host_path_not_allowed",
                    format!("mount host_path '{}' not allowed", mount.host_path),
                ));
            }
            if !path_allowed(&cap.local_guest_path_prefixes, &mount.guest_path) {
                return Ok(deny(
                    "mount_guest_path_not_allowed",
                    format!("mount guest_path '{}' not allowed", mount.guest_path),
                ));
            }
        }
    }

    if let Some(workdir) = local.workdir.as_ref() {
        if !path_allowed(&cap.workdir_prefixes, workdir) {
            return Ok(deny(
                "workdir_not_allowed",
                format!("workdir '{}' not allowed", workdir),
            ));
        }
    }

    if let Some(env) = local.env.as_ref() {
        if let Some(limit) = cap.max_env_vars {
            let count = env.len() as u64;
            if count > limit {
                return Ok(deny(
                    "env_limit_exceeded",
                    format!("env var count {count} exceeds max_env_vars {limit}"),
                ));
            }
        }
        for key in env.keys() {
            if !allowlist_contains(&cap.env_allow, key, |v| v.to_string()) {
                return Ok(deny(
                    "env_var_not_allowed",
                    format!("env var '{}' not allowed", key),
                ));
            }
        }
    }

    if let (Some(ttl), Some(max_ttl)) = (params.session_ttl_ns, cap.max_session_ttl_ns) {
        if ttl > max_ttl {
            return Ok(deny(
                "session_ttl_exceeded",
                format!("session_ttl_ns {ttl} exceeds max_session_ttl_ns {max_ttl}"),
            ));
        }
    }

    Ok(allow())
}

fn validate_exec(
    cap: &ProcessCapParams,
    params: ProcessExecParams,
) -> Result<CapCheckOutput, PureError> {
    if params.argv.is_empty() {
        return Ok(deny("argv_invalid", "argv must not be empty"));
    }

    if let Some(limit) = cap.max_argv_len {
        let count = params.argv.len() as u64;
        if count > limit {
            return Ok(deny(
                "argv_limit_exceeded",
                format!("argv length {count} exceeds max_argv_len {limit}"),
            ));
        }
    }

    if let Some(limit) = cap.max_arg_length {
        for arg in &params.argv {
            let len = arg.len() as u64;
            if len > limit {
                return Ok(deny(
                    "arg_length_exceeded",
                    format!("arg length {len} exceeds max_arg_length {limit}"),
                ));
            }
        }
    }

    if let (Some(timeout_ns), Some(max_timeout_ns)) = (params.timeout_ns, cap.max_exec_timeout_ns) {
        if timeout_ns > max_timeout_ns {
            return Ok(deny(
                "timeout_exceeded",
                format!("timeout_ns {timeout_ns} exceeds max_exec_timeout_ns {max_timeout_ns}"),
            ));
        }
    }

    if let Some(cwd) = params.cwd.as_ref() {
        if !path_allowed(&cap.workdir_prefixes, cwd) {
            return Ok(deny(
                "cwd_not_allowed",
                format!("cwd '{}' not allowed", cwd),
            ));
        }
    }

    if let Some(env_patch) = params.env_patch.as_ref() {
        if let Some(limit) = cap.max_env_vars {
            let count = env_patch.len() as u64;
            if count > limit {
                return Ok(deny(
                    "env_patch_limit_exceeded",
                    format!("env_patch var count {count} exceeds max_env_vars {limit}"),
                ));
            }
        }
        for key in env_patch.keys() {
            if !allowlist_contains(&cap.env_allow, key, |v| v.to_string()) {
                return Ok(deny(
                    "env_var_not_allowed",
                    format!("env var '{}' not allowed", key),
                ));
            }
        }
    }

    let output_mode = params.output_mode.unwrap_or_else(|| "auto".into());
    if !allowlist_contains(&cap.allowed_output_modes, &output_mode, |v| {
        v.to_ascii_lowercase()
    }) {
        return Ok(deny(
            "output_mode_not_allowed",
            format!("output_mode '{}' not allowed", output_mode),
        ));
    }

    Ok(allow())
}

fn validate_signal(
    cap: &ProcessCapParams,
    params: ProcessSignalParams,
) -> Result<CapCheckOutput, PureError> {
    if !allowlist_contains(&cap.allowed_signals, &params.signal, |v| {
        v.to_ascii_lowercase()
    }) {
        return Ok(deny(
            "signal_not_allowed",
            format!("signal '{}' not allowed", params.signal),
        ));
    }
    Ok(allow())
}

fn allow() -> CapCheckOutput {
    CapCheckOutput {
        constraints_ok: true,
        deny: None,
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
        if path == prefix || path.starts_with(prefix) {
            return true;
        }
    }
    false
}
