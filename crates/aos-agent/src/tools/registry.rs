use crate::contracts::{
    ToolAvailabilityRule, ToolExecutor, ToolMapper, ToolParallelismHint, ToolSpec,
};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const MAX_LLM_TOOL_NAME_LEN: usize = 64;

fn sha256_text(bytes: &[u8]) -> String {
    let mut out = String::from("sha256:");
    let digest = Sha256::digest(bytes);
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

pub fn is_valid_llm_tool_name(tool_name: &str) -> bool {
    if tool_name.is_empty() || tool_name.len() > MAX_LLM_TOOL_NAME_LEN {
        return false;
    }
    tool_name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

pub fn validate_tool_registry(registry: &BTreeMap<String, ToolSpec>) -> Result<(), String> {
    let mut llm_names = BTreeSet::new();
    for (key, spec) in registry {
        if key != &spec.tool_id {
            return Err(format!(
                "tool registry key '{}' does not match tool_id '{}'",
                key, spec.tool_id
            ));
        }
        if spec.tool_id.trim().is_empty() {
            return Err("tool_id must be non-empty".into());
        }
        if !is_valid_llm_tool_name(spec.tool_name.as_str()) {
            return Err(format!(
                "tool '{}' has invalid llm tool_name '{}'",
                spec.tool_id, spec.tool_name
            ));
        }
        if !llm_names.insert(spec.tool_name.clone()) {
            return Err(format!("duplicate llm tool_name '{}'", spec.tool_name));
        }
    }
    Ok(())
}

pub fn tool_definition_json(tool_name: &str, description: &str, args_schema_json: &str) -> Value {
    let parameters = serde_json::from_str::<Value>(args_schema_json).unwrap_or_else(|_| json!({}));
    json!({
        "name": tool_name,
        "description": description,
        "parameters": parameters,
    })
}

pub fn tool_definition_bytes(
    tool_name: &str,
    description: &str,
    args_schema_json: &str,
) -> Vec<u8> {
    serde_json::to_vec(&tool_definition_json(
        tool_name,
        description,
        args_schema_json,
    ))
    .unwrap_or_else(|_| b"{}".to_vec())
}

fn host_tool(
    tool_id: &str,
    tool_name: &str,
    description: &str,
    args_schema_json: &str,
    mapper: ToolMapper,
    requires_host_session: bool,
    hint: ToolParallelismHint,
) -> ToolSpec {
    assert!(
        is_valid_llm_tool_name(tool_name),
        "invalid llm tool name '{}'",
        tool_name
    );
    let tool_def_bytes = tool_definition_bytes(tool_name, description, args_schema_json);
    ToolSpec {
        tool_id: tool_id.to_string(),
        tool_name: tool_name.to_string(),
        tool_ref: sha256_text(&tool_def_bytes),
        description: description.to_string(),
        args_schema_json: args_schema_json.to_string(),
        mapper,
        executor: ToolExecutor::Effect {
            effect_kind: tool_id.to_string(),
            cap_slot: Some("host".into()),
        },
        availability_rules: if requires_host_session {
            vec![ToolAvailabilityRule::HostSessionReady]
        } else if mapper == ToolMapper::HostSessionOpen {
            vec![ToolAvailabilityRule::HostSessionNotReady]
        } else {
            vec![ToolAvailabilityRule::Always]
        },
        parallelism_hint: hint,
    }
}

fn effect_tool(
    tool_id: &str,
    tool_name: &str,
    description: &str,
    args_schema_json: &str,
    mapper: ToolMapper,
    cap_slot: &str,
    hint: ToolParallelismHint,
) -> ToolSpec {
    assert!(
        is_valid_llm_tool_name(tool_name),
        "invalid llm tool name '{}'",
        tool_name
    );
    let tool_def_bytes = tool_definition_bytes(tool_name, description, args_schema_json);
    ToolSpec {
        tool_id: tool_id.to_string(),
        tool_name: tool_name.to_string(),
        tool_ref: sha256_text(&tool_def_bytes),
        description: description.to_string(),
        args_schema_json: args_schema_json.to_string(),
        mapper,
        executor: ToolExecutor::Effect {
            effect_kind: tool_id.to_string(),
            cap_slot: Some(cap_slot.into()),
        },
        availability_rules: vec![ToolAvailabilityRule::Always],
        parallelism_hint: hint,
    }
}

pub fn default_tool_registry() -> BTreeMap<String, ToolSpec> {
    let mut registry = BTreeMap::new();

    let tools = [
        host_tool(
            "host.session.open",
            "open_session",
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
            "shell",
            "Execute a command in a host session.",
            r#"{"type":"object","required":["argv"],"properties":{"argv":{"type":"array","items":{"type":"string"}},"cwd":{"type":"string"},"timeout_ns":{"type":"integer"},"env_patch":{"type":"object"},"stdin_ref":{"type":"string"}}}"#,
            ToolMapper::HostExec,
            true,
            ToolParallelismHint {
                parallel_safe: false,
                resource_key: Some("host.exec".into()),
            },
        ),
        host_tool(
            "host.session.signal",
            "signal_session",
            "Send a signal to a host session.",
            r#"{"type":"object","required":["signal"],"properties":{"signal":{"type":"string"},"grace_timeout_ns":{"type":"integer"}}}"#,
            ToolMapper::HostSessionSignal,
            true,
            ToolParallelismHint {
                parallel_safe: false,
                resource_key: Some("host.session".into()),
            },
        ),
        host_tool(
            "host.fs.read_file",
            "read_file",
            "Read a file from the host filesystem.",
            r#"{"type":"object","required":["path"],"properties":{"path":{"type":"string"},"offset_bytes":{"type":"integer"},"max_bytes":{"type":"integer"},"encoding":{"type":"string"}}}"#,
            ToolMapper::HostFsReadFile,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        host_tool(
            "host.fs.write_file",
            "write_file",
            "Write file contents on the host filesystem.",
            r#"{"type":"object","required":["path"],"properties":{"path":{"type":"string"},"text":{"type":"string"},"blob_ref":{"type":"string"},"create_parents":{"type":"boolean"},"mode":{"type":"string"}}}"#,
            ToolMapper::HostFsWriteFile,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: Some("host.fs.write".into()),
            },
        ),
        host_tool(
            "host.fs.edit_file",
            "edit_file",
            "Replace text in a file.",
            r#"{"type":"object","required":["path","old_string","new_string"],"properties":{"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}}}"#,
            ToolMapper::HostFsEditFile,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: Some("host.fs.write".into()),
            },
        ),
        host_tool(
            "host.fs.apply_patch",
            "apply_patch",
            "Apply a unified patch to files.",
            r#"{"type":"object","required":["patch"],"properties":{"patch":{"type":"string"},"dry_run":{"type":"boolean"}}}"#,
            ToolMapper::HostFsApplyPatch,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: Some("host.fs.write".into()),
            },
        ),
        host_tool(
            "host.fs.grep",
            "grep",
            "Search file contents by regex/text.",
            r#"{"type":"object","required":["pattern"],"properties":{"pattern":{"type":"string"},"path":{"type":"string"},"glob_filter":{"type":"string"},"case_insensitive":{"type":"boolean"},"max_results":{"type":"integer"}}}"#,
            ToolMapper::HostFsGrep,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        host_tool(
            "host.fs.glob",
            "glob",
            "List files matching a glob pattern.",
            r#"{"type":"object","required":["pattern"],"properties":{"pattern":{"type":"string"},"path":{"type":"string"},"max_results":{"type":"integer"}}}"#,
            ToolMapper::HostFsGlob,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        host_tool(
            "host.fs.stat",
            "stat",
            "Read metadata for a filesystem path.",
            r#"{"type":"object","required":["path"],"properties":{"path":{"type":"string"}}}"#,
            ToolMapper::HostFsStat,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        host_tool(
            "host.fs.exists",
            "exists",
            "Check whether a path exists.",
            r#"{"type":"object","required":["path"],"properties":{"path":{"type":"string"}}}"#,
            ToolMapper::HostFsExists,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        host_tool(
            "host.fs.list_dir",
            "list_dir",
            "List directory entries.",
            r#"{"type":"object","properties":{"path":{"type":"string"},"max_results":{"type":"integer"}}}"#,
            ToolMapper::HostFsListDir,
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        effect_tool(
            "introspect.manifest",
            "inspect_world",
            "Inspect world summary, modules, effects, routing, and manifest metadata.",
            r#"{"type":"object","additionalProperties":false,"properties":{}}"#,
            ToolMapper::InspectWorld,
            "query",
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        effect_tool(
            "introspect.workflow_state",
            "inspect_workflow",
            "Inspect a workflow's current state or list its cells.",
            r#"{"type":"object","required":["workflow"],"properties":{"workflow":{"type":"string"},"view":{"type":"string","enum":["state","cells"]},"cell_key":{"type":"object","required":["encoding","value"],"properties":{"encoding":{"type":"string","enum":["utf8","hex"]},"value":{"type":"string"}},"additionalProperties":false}},"additionalProperties":false}"#,
            ToolMapper::InspectWorkflow,
            "query",
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        effect_tool(
            "workspace.inspect",
            "workspace_inspect",
            "Resolve a workspace to its current or requested root, or inspect a specific root hash.",
            r#"{"type":"object","additionalProperties":false,"properties":{"workspace":{"type":"string"},"version":{"type":"integer","minimum":0},"root_hash":{"type":"string"}}}"#,
            ToolMapper::WorkspaceInspect,
            "workspace",
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        effect_tool(
            "workspace.list",
            "workspace_list",
            "List workspaces or list entries in a workspace tree.",
            r#"{"type":"object","additionalProperties":false,"properties":{"workspace":{"type":"string"},"version":{"type":"integer","minimum":0},"root_hash":{"type":"string"},"path":{"type":"string"},"scope":{"type":"string","enum":["dir","subtree"]},"limit":{"type":"integer","minimum":0}}}"#,
            ToolMapper::WorkspaceList,
            "workspace",
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        effect_tool(
            "workspace.read",
            "workspace_read",
            "Read workspace entry metadata and file content.",
            r#"{"type":"object","required":["path"],"additionalProperties":false,"properties":{"workspace":{"type":"string"},"version":{"type":"integer","minimum":0},"root_hash":{"type":"string"},"path":{"type":"string"},"range":{"type":"object","required":["start","end"],"properties":{"start":{"type":"integer","minimum":0},"end":{"type":"integer","minimum":0}},"additionalProperties":false}}}"#,
            ToolMapper::WorkspaceRead,
            "workspace",
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        effect_tool(
            "workspace.apply",
            "workspace_apply",
            "Apply writes and removals to a workspace tree and return a new root hash.",
            r#"{"type":"object","required":["operations"],"additionalProperties":false,"properties":{"workspace":{"type":"string"},"version":{"type":"integer","minimum":0},"root_hash":{"type":"string"},"operations":{"type":"array","items":{"type":"object","required":["op","path"],"properties":{"op":{"type":"string","enum":["write","remove"]},"path":{"type":"string"},"text":{"type":"string"},"bytes_b64":{"type":"string"},"blob_hash":{"type":"string"},"mode":{"type":"integer","minimum":0}},"additionalProperties":false}}}}"#,
            ToolMapper::WorkspaceApply,
            "workspace",
            ToolParallelismHint {
                parallel_safe: false,
                resource_key: Some("workspace.apply".into()),
            },
        ),
        effect_tool(
            "workspace.diff",
            "workspace_diff",
            "Diff two workspace roots or named workspace versions.",
            r#"{"type":"object","required":["left","right"],"additionalProperties":false,"properties":{"left":{"type":"object","additionalProperties":false,"properties":{"workspace":{"type":"string"},"version":{"type":"integer","minimum":0},"root_hash":{"type":"string"}}},"right":{"type":"object","additionalProperties":false,"properties":{"workspace":{"type":"string"},"version":{"type":"integer","minimum":0},"root_hash":{"type":"string"}}},"prefix":{"type":"string"}}}"#,
            ToolMapper::WorkspaceDiff,
            "workspace",
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        ToolSpec {
            tool_id: "workspace.commit".into(),
            tool_name: "workspace_commit".into(),
            tool_ref: sha256_text(&tool_definition_bytes(
                "workspace_commit",
                "Publish a root hash as the next version of a named workspace.",
                r#"{"type":"object","required":["workspace","root_hash"],"additionalProperties":false,"properties":{"workspace":{"type":"string"},"root_hash":{"type":"string"},"expected_head":{"type":"integer","minimum":0},"owner":{"type":"string"}}}"#,
            )),
            description:
                "Publish a root hash as the next version of a named workspace.".into(),
            args_schema_json: r#"{"type":"object","required":["workspace","root_hash"],"additionalProperties":false,"properties":{"workspace":{"type":"string"},"root_hash":{"type":"string"},"expected_head":{"type":"integer","minimum":0},"owner":{"type":"string"}}}"#.into(),
            mapper: ToolMapper::WorkspaceCommit,
            executor: ToolExecutor::DomainEvent {
                schema: "sys/WorkspaceCommit@1".into(),
            },
            availability_rules: vec![ToolAvailabilityRule::Always],
            parallelism_hint: ToolParallelismHint {
                parallel_safe: false,
                resource_key: Some("workspace.commit".into()),
            },
        },
    ];

    for tool in tools {
        registry.insert(tool.tool_id.clone(), tool);
    }

    validate_tool_registry(&registry).expect("default tool registry must be valid");
    registry
}

pub fn default_tool_profiles() -> BTreeMap<String, Vec<String>> {
    let common = vec![
        "introspect.manifest".into(),
        "introspect.workflow_state".into(),
        "workspace.inspect".into(),
        "workspace.list".into(),
        "workspace.read".into(),
        "workspace.apply".into(),
        "workspace.diff".into(),
        "workspace.commit".into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_tool_name_validation_accepts_expected_characters() {
        assert!(is_valid_llm_tool_name("read_file"));
        assert!(is_valid_llm_tool_name("apply-patch"));
        assert!(!is_valid_llm_tool_name("host.fs.read_file"));
        assert!(!is_valid_llm_tool_name(""));
    }

    #[test]
    fn default_registry_uses_unique_valid_llm_names() {
        let registry = default_tool_registry();
        assert!(validate_tool_registry(&registry).is_ok());
    }

    #[test]
    fn provider_profiles_select_different_mutation_tools() {
        let profiles = default_tool_profiles();
        let openai = profiles.get("openai").expect("openai profile");
        let anthropic = profiles.get("anthropic").expect("anthropic profile");
        assert!(openai.iter().any(|id| id == "host.fs.apply_patch"));
        assert!(!openai.iter().any(|id| id == "host.fs.edit_file"));
        assert!(anthropic.iter().any(|id| id == "host.fs.edit_file"));
        assert!(!anthropic.iter().any(|id| id == "host.fs.apply_patch"));
    }
}
