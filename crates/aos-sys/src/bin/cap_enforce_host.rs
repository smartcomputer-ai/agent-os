//! Cap enforcer for host effects (`sys/CapEnforceHost@1`).

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

aos_pure!(CapEnforceHost);

#[derive(Default)]
struct CapEnforceHost;

#[derive(Deserialize)]
struct HostCapParams {
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

    allowed_fs_ops: Option<Vec<String>>,
    fs_roots: Option<Vec<String>>,
    max_read_bytes: Option<u64>,
    max_write_bytes: Option<u64>,
    max_patch_bytes: Option<u64>,
    max_inline_bytes: Option<u64>,
    max_grep_results: Option<u64>,
    max_glob_results: Option<u64>,
    max_scan_files: Option<u64>,
    max_scan_bytes: Option<u64>,
    allowed_patch_formats: Option<Vec<String>>,
    max_changed_files: Option<u64>,
    max_edit_replacements: Option<u64>,
    follow_symlinks: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostTarget {
    local: Option<HostLocalTarget>,
}

#[derive(Deserialize)]
struct HostLocalTarget {
    mounts: Option<Vec<HostMount>>,
    workdir: Option<String>,
    env: Option<BTreeMap<String, String>>,
    network_mode: String,
}

#[derive(Deserialize)]
struct HostMount {
    host_path: String,
    guest_path: String,
    mode: String,
}

#[derive(Deserialize)]
struct HostSessionOpenParams {
    target: HostTarget,
    session_ttl_ns: Option<u64>,
}

#[derive(Deserialize)]
struct HostExecParams {
    argv: Vec<String>,
    cwd: Option<String>,
    timeout_ns: Option<u64>,
    env_patch: Option<BTreeMap<String, String>>,
    output_mode: Option<String>,
}

#[derive(Deserialize)]
struct HostSignalParams {
    signal: String,
}

#[derive(Deserialize)]
struct HostFsReadFileParams {
    path: String,
    max_bytes: Option<u64>,
    output_mode: Option<String>,
}

#[derive(Deserialize)]
struct HostInlineText {
    text: String,
}

#[derive(Deserialize)]
struct HostInlineBytes {
    #[serde(with = "serde_bytes")]
    bytes: Vec<u8>,
}

#[derive(Deserialize)]
struct HostBlobRefInput {
    blob_ref: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum HostFileContentInput {
    InlineText { inline_text: HostInlineText },
    InlineBytes { inline_bytes: HostInlineBytes },
    BlobRef { blob_ref: HostBlobRefInput },
}

#[derive(Deserialize)]
struct HostFsWriteFileParams {
    path: String,
    content: HostFileContentInput,
    mode: Option<String>,
}

#[derive(Deserialize)]
struct HostFsEditFileParams {
    path: String,
    old_string: String,
    replace_all: Option<bool>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum HostPatchInput {
    InlineText { inline_text: HostInlineText },
    BlobRef { blob_ref: HostBlobRefInput },
}

#[derive(Deserialize)]
struct HostFsApplyPatchParams {
    patch: HostPatchInput,
    patch_format: Option<String>,
}

#[derive(Deserialize)]
struct HostFsGrepParams {
    path: Option<String>,
    max_results: Option<u64>,
    output_mode: Option<String>,
}

#[derive(Deserialize)]
struct HostFsGlobParams {
    path: Option<String>,
    max_results: Option<u64>,
    output_mode: Option<String>,
}

#[derive(Deserialize)]
struct HostFsStatParams {
    path: String,
}

#[derive(Deserialize)]
struct HostFsExistsParams {
    path: String,
}

#[derive(Deserialize)]
struct HostFsListDirParams {
    path: Option<String>,
    max_results: Option<u64>,
    output_mode: Option<String>,
}

impl PureModule for CapEnforceHost {
    type Input = CapCheckInput;
    type Output = CapCheckOutput;

    fn run(
        &mut self,
        input: Self::Input,
        _ctx: Option<&PureContext>,
    ) -> Result<Self::Output, PureError> {
        let cap_params: HostCapParams = match decode_cbor(&input.cap_params) {
            Ok(value) => value,
            Err(err) => {
                return Ok(deny("cap_params_invalid", err.to_string()));
            }
        };

        match input.effect_kind.as_str() {
            "host.session.open" => {
                let params: HostSessionOpenParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => {
                        return Ok(deny("effect_params_invalid", err.to_string()));
                    }
                };
                validate_open(&cap_params, params)
            }
            "host.exec" => {
                let params: HostExecParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => {
                        return Ok(deny("effect_params_invalid", err.to_string()));
                    }
                };
                validate_exec(&cap_params, params)
            }
            "host.session.signal" => {
                let params: HostSignalParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => {
                        return Ok(deny("effect_params_invalid", err.to_string()));
                    }
                };
                validate_signal(&cap_params, params)
            }
            "host.fs.read_file" => {
                let params: HostFsReadFileParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => return Ok(deny("effect_params_invalid", err.to_string())),
                };
                validate_read_file(&cap_params, params)
            }
            "host.fs.write_file" => {
                let params: HostFsWriteFileParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => return Ok(deny("effect_params_invalid", err.to_string())),
                };
                validate_write_file(&cap_params, params)
            }
            "host.fs.edit_file" => {
                let params: HostFsEditFileParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => return Ok(deny("effect_params_invalid", err.to_string())),
                };
                validate_edit_file(&cap_params, params)
            }
            "host.fs.apply_patch" => {
                let params: HostFsApplyPatchParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => return Ok(deny("effect_params_invalid", err.to_string())),
                };
                validate_apply_patch(&cap_params, params)
            }
            "host.fs.grep" => {
                let params: HostFsGrepParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => return Ok(deny("effect_params_invalid", err.to_string())),
                };
                validate_grep(&cap_params, params)
            }
            "host.fs.glob" => {
                let params: HostFsGlobParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => return Ok(deny("effect_params_invalid", err.to_string())),
                };
                validate_glob(&cap_params, params)
            }
            "host.fs.stat" => {
                let params: HostFsStatParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => return Ok(deny("effect_params_invalid", err.to_string())),
                };
                validate_stat(&cap_params, params)
            }
            "host.fs.exists" => {
                let params: HostFsExistsParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => return Ok(deny("effect_params_invalid", err.to_string())),
                };
                validate_exists(&cap_params, params)
            }
            "host.fs.list_dir" => {
                let params: HostFsListDirParams = match decode_cbor(&input.effect_params) {
                    Ok(value) => value,
                    Err(err) => return Ok(deny("effect_params_invalid", err.to_string())),
                };
                validate_list_dir(&cap_params, params)
            }
            other => Ok(deny(
                "effect_kind_mismatch",
                format!("enforcer 'sys/CapEnforceHost@1' cannot handle '{}'", other),
            )),
        }
    }
}

fn validate_open(
    cap: &HostCapParams,
    params: HostSessionOpenParams,
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

fn validate_exec(cap: &HostCapParams, params: HostExecParams) -> Result<CapCheckOutput, PureError> {
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
    cap: &HostCapParams,
    params: HostSignalParams,
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

fn validate_read_file(
    cap: &HostCapParams,
    params: HostFsReadFileParams,
) -> Result<CapCheckOutput, PureError> {
    require_fs_op(cap, "read")?;
    if !path_allowed(&cap.fs_roots, &params.path) {
        return Ok(deny(
            "path_not_allowed",
            format!("path '{}' not allowed", params.path),
        ));
    }

    if let (Some(limit), Some(requested)) = (cap.max_read_bytes, params.max_bytes) {
        if requested > limit {
            return Ok(deny(
                "max_read_bytes_exceeded",
                format!("max_bytes {requested} exceeds max_read_bytes {limit}"),
            ));
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

fn validate_write_file(
    cap: &HostCapParams,
    params: HostFsWriteFileParams,
) -> Result<CapCheckOutput, PureError> {
    require_fs_op(cap, "write")?;
    if !path_allowed(&cap.fs_roots, &params.path) {
        return Ok(deny(
            "path_not_allowed",
            format!("path '{}' not allowed", params.path),
        ));
    }

    let mode = params.mode.unwrap_or_else(|| "overwrite".into());
    if mode != "overwrite" && mode != "create_new" {
        return Ok(deny("invalid_mode", format!("unsupported mode '{}'", mode)));
    }

    if let Some(limit) = cap.max_write_bytes {
        let known_size = match params.content {
            HostFileContentInput::InlineText { inline_text } => Some(inline_text.text.len() as u64),
            HostFileContentInput::InlineBytes { inline_bytes } => {
                Some(inline_bytes.bytes.len() as u64)
            }
            HostFileContentInput::BlobRef { blob_ref } => {
                let _ = blob_ref.blob_ref;
                None
            }
        };
        if let Some(size) = known_size {
            if size > limit {
                return Ok(deny(
                    "max_write_bytes_exceeded",
                    format!("content size {size} exceeds max_write_bytes {limit}"),
                ));
            }
        }
    }

    Ok(allow())
}

fn validate_edit_file(
    cap: &HostCapParams,
    params: HostFsEditFileParams,
) -> Result<CapCheckOutput, PureError> {
    require_fs_op(cap, "edit")?;
    if !path_allowed(&cap.fs_roots, &params.path) {
        return Ok(deny(
            "path_not_allowed",
            format!("path '{}' not allowed", params.path),
        ));
    }

    if params.old_string.is_empty() {
        return Ok(deny(
            "invalid_input_empty_old_string",
            "old_string must not be empty",
        ));
    }

    if let Some(limit) = cap.max_edit_replacements {
        if limit == 0 && params.replace_all.unwrap_or(false) {
            return Ok(deny(
                "max_edit_replacements_exceeded",
                "replace_all is not allowed when max_edit_replacements is 0",
            ));
        }
    }

    Ok(allow())
}

fn validate_apply_patch(
    cap: &HostCapParams,
    params: HostFsApplyPatchParams,
) -> Result<CapCheckOutput, PureError> {
    require_fs_op(cap, "patch")?;

    let patch_format = params.patch_format.unwrap_or_else(|| "v4a".into());
    if !allowlist_contains(&cap.allowed_patch_formats, &patch_format, |v| {
        v.to_ascii_lowercase()
    }) {
        return Ok(deny(
            "patch_format_not_allowed",
            format!("patch_format '{}' not allowed", patch_format),
        ));
    }

    if let Some(limit) = cap.max_patch_bytes {
        match params.patch {
            HostPatchInput::InlineText { inline_text } => {
                let size = inline_text.text.len() as u64;
                if size > limit {
                    return Ok(deny(
                        "max_patch_bytes_exceeded",
                        format!("patch size {size} exceeds max_patch_bytes {limit}"),
                    ));
                }
            }
            HostPatchInput::BlobRef { blob_ref } => {
                let _ = blob_ref.blob_ref;
            }
        }
    }

    Ok(allow())
}

fn validate_grep(
    cap: &HostCapParams,
    params: HostFsGrepParams,
) -> Result<CapCheckOutput, PureError> {
    require_fs_op(cap, "search")?;
    if let Some(path) = params.path.as_ref() {
        if !path_allowed(&cap.fs_roots, path) {
            return Ok(deny(
                "path_not_allowed",
                format!("path '{}' not allowed", path),
            ));
        }
    }

    if let (Some(limit), Some(requested)) = (cap.max_grep_results, params.max_results) {
        if requested > limit {
            return Ok(deny(
                "max_grep_results_exceeded",
                format!("max_results {requested} exceeds max_grep_results {limit}"),
            ));
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

fn validate_glob(
    cap: &HostCapParams,
    params: HostFsGlobParams,
) -> Result<CapCheckOutput, PureError> {
    require_fs_op(cap, "list")?;
    if let Some(path) = params.path.as_ref() {
        if !path_allowed(&cap.fs_roots, path) {
            return Ok(deny(
                "path_not_allowed",
                format!("path '{}' not allowed", path),
            ));
        }
    }

    if let (Some(limit), Some(requested)) = (cap.max_glob_results, params.max_results) {
        if requested > limit {
            return Ok(deny(
                "max_glob_results_exceeded",
                format!("max_results {requested} exceeds max_glob_results {limit}"),
            ));
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

fn validate_stat(
    cap: &HostCapParams,
    params: HostFsStatParams,
) -> Result<CapCheckOutput, PureError> {
    require_fs_op(cap, "stat")?;
    if !path_allowed(&cap.fs_roots, &params.path) {
        return Ok(deny(
            "path_not_allowed",
            format!("path '{}' not allowed", params.path),
        ));
    }
    Ok(allow())
}

fn validate_exists(
    cap: &HostCapParams,
    params: HostFsExistsParams,
) -> Result<CapCheckOutput, PureError> {
    require_fs_op(cap, "stat")?;
    if !path_allowed(&cap.fs_roots, &params.path) {
        return Ok(deny(
            "path_not_allowed",
            format!("path '{}' not allowed", params.path),
        ));
    }
    Ok(allow())
}

fn validate_list_dir(
    cap: &HostCapParams,
    params: HostFsListDirParams,
) -> Result<CapCheckOutput, PureError> {
    require_fs_op(cap, "list")?;
    if let Some(path) = params.path.as_ref() {
        if !path_allowed(&cap.fs_roots, path) {
            return Ok(deny(
                "path_not_allowed",
                format!("path '{}' not allowed", path),
            ));
        }
    }

    if let (Some(limit), Some(requested)) = (cap.max_glob_results, params.max_results) {
        if requested > limit {
            return Ok(deny(
                "max_glob_results_exceeded",
                format!("max_results {requested} exceeds max_glob_results {limit}"),
            ));
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

fn require_fs_op(cap: &HostCapParams, op: &str) -> Result<CapCheckOutput, PureError> {
    if !allowlist_contains(&cap.allowed_fs_ops, op, |v| v.to_ascii_lowercase()) {
        return Ok(deny(
            "fs_op_not_allowed",
            format!("host fs operation '{}' not allowed", op),
        ));
    }

    if let Some(mode) = cap.follow_symlinks.as_ref() {
        if mode != "deny" && mode != "within_root_only" && mode != "allow" {
            return Ok(deny(
                "invalid_follow_symlinks",
                format!("unsupported follow_symlinks '{}')", mode),
            ));
        }
    }

    let _ = cap.max_inline_bytes;
    let _ = cap.max_scan_files;
    let _ = cap.max_scan_bytes;
    let _ = cap.max_changed_files;

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
