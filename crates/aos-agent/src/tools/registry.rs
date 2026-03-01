use crate::contracts::{
    ToolAvailabilityRule, ToolExecutor, ToolMapper, ToolParallelismHint, ToolSpec,
};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

fn pseudo_hash(seed: &str) -> String {
    let mut out = String::from("sha256:");
    let digest = Sha256::digest(seed.as_bytes());
    for byte in digest {
        let hi = byte >> 4;
        let lo = byte & 0x0f;
        out.push(nibble_to_hex(hi));
        out.push(nibble_to_hex(lo));
    }
    out
}

const fn nibble_to_hex(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        _ => (b'a' + (n - 10)) as char,
    }
}

fn host_tool(
    tool_name: &str,
    description: &str,
    args_schema_json: &str,
    mapper: ToolMapper,
    requires_host_session: bool,
    hint: ToolParallelismHint,
) -> ToolSpec {
    ToolSpec {
        tool_name: tool_name.to_string(),
        tool_ref: pseudo_hash(tool_name),
        description: description.to_string(),
        args_schema_json: args_schema_json.to_string(),
        mapper,
        executor: ToolExecutor::Effect {
            effect_kind: tool_name.to_string(),
            cap_slot: Some("host".into()),
        },
        availability_rules: if requires_host_session {
            vec![ToolAvailabilityRule::HostSessionReady]
        } else {
            vec![ToolAvailabilityRule::Always]
        },
        parallelism_hint: hint,
    }
}

pub fn default_tool_registry() -> BTreeMap<String, ToolSpec> {
    let mut registry = BTreeMap::new();

    let tools = [
        host_tool(
            "host.session.open",
            "Open a host session and return session_id.",
            r#"{"type":"object","properties":{"target":{"type":"object"},"session_ttl_ns":{"type":"integer"},"labels":{"type":"object"}}}"#,
            ToolMapper::HostSessionOpen,
            false,
            ToolParallelismHint {
                parallel_safe: false,
                resource_key: Some("host.session".into()),
            },
        ),
        host_tool(
            "host.exec",
            "Execute a command in a host session.",
            r#"{"type":"object","required":["argv"],"properties":{"session_id":{"type":"string"},"argv":{"type":"array","items":{"type":"string"}},"cwd":{"type":"string"},"timeout_ns":{"type":"integer"},"env_patch":{"type":"object"},"stdin_ref":{"type":"string"},"output_mode":{"type":"string"}}}"#,
            ToolMapper::HostExec,
            true,
            ToolParallelismHint {
                parallel_safe: false,
                resource_key: Some("host.exec".into()),
            },
        ),
        host_tool(
            "host.session.signal",
            "Send a signal to a host session.",
            r#"{"type":"object","required":["signal"],"properties":{"session_id":{"type":"string"},"signal":{"type":"string"},"grace_timeout_ns":{"type":"integer"}}}"#,
            ToolMapper::HostSessionSignal,
            true,
            ToolParallelismHint {
                parallel_safe: false,
                resource_key: Some("host.session".into()),
            },
        ),
        host_tool(
            "host.fs.read_file",
            "Read a file from the host filesystem.",
            r#"{"type":"object","required":["path"],"properties":{"session_id":{"type":"string"},"path":{"type":"string"},"offset_bytes":{"type":"integer"},"max_bytes":{"type":"integer"},"encoding":{"type":"string"},"output_mode":{"type":"string"}}}"#,
            ToolMapper::HostFsReadFile,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        host_tool(
            "host.fs.write_file",
            "Write file contents on the host filesystem.",
            r#"{"type":"object","required":["path"],"properties":{"session_id":{"type":"string"},"path":{"type":"string"},"text":{"type":"string"},"blob_ref":{"type":"string"},"create_parents":{"type":"boolean"},"mode":{"type":"string"}}}"#,
            ToolMapper::HostFsWriteFile,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: Some("host.fs.write".into()),
            },
        ),
        host_tool(
            "host.fs.edit_file",
            "Replace text in a file.",
            r#"{"type":"object","required":["path","old_string","new_string"],"properties":{"session_id":{"type":"string"},"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}}}"#,
            ToolMapper::HostFsEditFile,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: Some("host.fs.write".into()),
            },
        ),
        host_tool(
            "host.fs.apply_patch",
            "Apply a unified patch to files.",
            r#"{"type":"object","required":["patch"],"properties":{"session_id":{"type":"string"},"patch":{"type":"string"},"patch_format":{"type":"string"},"dry_run":{"type":"boolean"}}}"#,
            ToolMapper::HostFsApplyPatch,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: Some("host.fs.write".into()),
            },
        ),
        host_tool(
            "host.fs.grep",
            "Search file contents by regex/text.",
            r#"{"type":"object","required":["pattern"],"properties":{"session_id":{"type":"string"},"pattern":{"type":"string"},"path":{"type":"string"},"glob_filter":{"type":"string"},"case_insensitive":{"type":"boolean"},"max_results":{"type":"integer"},"output_mode":{"type":"string"}}}"#,
            ToolMapper::HostFsGrep,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        host_tool(
            "host.fs.glob",
            "List files matching a glob pattern.",
            r#"{"type":"object","required":["pattern"],"properties":{"session_id":{"type":"string"},"pattern":{"type":"string"},"path":{"type":"string"},"max_results":{"type":"integer"},"output_mode":{"type":"string"}}}"#,
            ToolMapper::HostFsGlob,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        host_tool(
            "host.fs.stat",
            "Read metadata for a filesystem path.",
            r#"{"type":"object","required":["path"],"properties":{"session_id":{"type":"string"},"path":{"type":"string"}}}"#,
            ToolMapper::HostFsStat,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        host_tool(
            "host.fs.exists",
            "Check whether a path exists.",
            r#"{"type":"object","required":["path"],"properties":{"session_id":{"type":"string"},"path":{"type":"string"}}}"#,
            ToolMapper::HostFsExists,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        host_tool(
            "host.fs.list_dir",
            "List directory entries.",
            r#"{"type":"object","properties":{"session_id":{"type":"string"},"path":{"type":"string"},"max_results":{"type":"integer"},"output_mode":{"type":"string"}}}"#,
            ToolMapper::HostFsListDir,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
    ];

    for tool in tools {
        registry.insert(tool.tool_name.clone(), tool);
    }
    registry
}

pub fn default_tool_profiles() -> BTreeMap<String, Vec<String>> {
    let common = vec![
        "host.session.open".into(),
        "host.exec".into(),
        "host.fs.read_file".into(),
        "host.fs.write_file".into(),
        "host.fs.grep".into(),
        "host.fs.glob".into(),
        "host.fs.stat".into(),
        "host.fs.exists".into(),
        "host.fs.list_dir".into(),
    ];

    let mut profiles = BTreeMap::new();

    let mut openai = common.clone();
    openai.push("host.fs.apply_patch".into());
    profiles.insert("openai".into(), openai.clone());
    profiles.insert("default".into(), openai);

    let mut anthropic = common.clone();
    anthropic.push("host.fs.edit_file".into());
    profiles.insert("anthropic".into(), anthropic.clone());
    profiles.insert("gemini".into(), anthropic);

    profiles
}

pub fn default_tool_profile_for_provider(provider: &str) -> String {
    let normalized = provider.trim().to_ascii_lowercase();
    if normalized.contains("anthropic") {
        "anthropic".into()
    } else if normalized.contains("gemini") {
        "gemini".into()
    } else {
        "openai".into()
    }
}
