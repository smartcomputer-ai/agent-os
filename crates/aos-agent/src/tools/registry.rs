use crate::contracts::{ToolExecutor, ToolMapper, ToolParallelismHint, ToolSpec};
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
    _requires_host_session: bool,
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
            effect: tool_id.to_string(),
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
            effect: tool_id.to_string(),
        },
        parallelism_hint: hint,
    }
}

pub fn default_tool_registry() -> BTreeMap<String, ToolSpec> {
    BTreeMap::new()
}

pub fn default_tool_profiles() -> BTreeMap<String, Vec<String>> {
    BTreeMap::new()
}

pub fn default_tool_profile_for_provider(_provider: &str) -> String {
    String::new()
}

pub fn tool_bundle_inspect() -> Vec<ToolSpec> {
    tools_by_id(&["introspect.manifest", "introspect.workflow_state"])
}

pub fn tool_bundle_host_session() -> Vec<ToolSpec> {
    tools_by_id(&["host.session.open", "host.session.signal"])
}

pub fn tool_bundle_host_fs() -> Vec<ToolSpec> {
    tools_by_id(&[
        "host.exec",
        "host.fs.read_file",
        "host.fs.write_file",
        "host.fs.edit_file",
        "host.fs.apply_patch",
        "host.fs.grep",
        "host.fs.glob",
        "host.fs.stat",
        "host.fs.exists",
        "host.fs.list_dir",
    ])
}

pub fn tool_bundle_host_local() -> Vec<ToolSpec> {
    let mut tools = tool_bundle_host_session();
    tools.extend(tool_bundle_host_fs());
    tools
}

pub fn tool_bundle_host_sandbox() -> Vec<ToolSpec> {
    let mut tools = tool_bundle_host_session();
    tools.extend(tool_bundle_host_fs());
    tools
}

pub fn tool_bundle_workspace() -> Vec<ToolSpec> {
    tools_by_id(&[
        "workspace.inspect",
        "workspace.list",
        "workspace.read",
        "workspace.apply",
        "workspace.diff",
        "workspace.commit",
    ])
}

pub fn local_coding_agent_tool_registry() -> BTreeMap<String, ToolSpec> {
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
            "Inspect world summary, modules, workflows, effects, routing, and manifest metadata.",
            r#"{"type":"object","additionalProperties":false,"properties":{}}"#,
            ToolMapper::InspectWorld,
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
            parallelism_hint: ToolParallelismHint {
                parallel_safe: false,
                resource_key: Some("workspace.commit".into()),
            },
        },
    ];

    for tool in tools {
        registry.insert(tool.tool_id.clone(), tool);
    }

    validate_tool_registry(&registry).expect("local coding agent tool registry must be valid");
    registry
}

fn tools_by_id(ids: &[&str]) -> Vec<ToolSpec> {
    let registry = local_coding_agent_tool_registry();
    ids.iter()
        .filter_map(|id| registry.get(*id).cloned())
        .collect()
}

pub fn local_coding_agent_tool_profiles() -> BTreeMap<String, Vec<String>> {
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

pub fn local_coding_agent_tool_profile_for_provider(provider: &str) -> String {
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
    fn default_registry_is_empty() {
        let registry = default_tool_registry();
        assert!(registry.is_empty());
        assert!(default_tool_profiles().is_empty());
        assert!(default_tool_profile_for_provider("openai").is_empty());
    }

    #[test]
    fn local_coding_registry_uses_unique_valid_llm_names() {
        let registry = local_coding_agent_tool_registry();
        assert!(validate_tool_registry(&registry).is_ok());
    }

    #[test]
    fn local_coding_provider_profiles_select_different_mutation_tools() {
        let profiles = local_coding_agent_tool_profiles();
        let openai = profiles.get("openai").expect("openai profile");
        let anthropic = profiles.get("anthropic").expect("anthropic profile");
        assert!(openai.iter().any(|id| id == "host.fs.apply_patch"));
        assert!(!openai.iter().any(|id| id == "host.fs.edit_file"));
        assert!(anthropic.iter().any(|id| id == "host.fs.edit_file"));
        assert!(!anthropic.iter().any(|id| id == "host.fs.apply_patch"));
    }

    #[test]
    fn bundle_constructors_are_independently_selectable() {
        let inspect = tool_bundle_inspect();
        assert!(
            inspect
                .iter()
                .any(|tool| tool.tool_id == "introspect.manifest")
        );
        assert!(!inspect.iter().any(|tool| tool.tool_id == "host.exec"));

        let host = tool_bundle_host_local();
        assert!(host.iter().any(|tool| tool.tool_id == "host.exec"));
        assert!(!host.iter().any(|tool| tool.tool_id == "workspace.read"));

        let sandbox = tool_bundle_host_sandbox();
        assert!(
            sandbox
                .iter()
                .any(|tool| tool.tool_id == "host.session.open")
        );
        assert!(sandbox.iter().any(|tool| tool.tool_id == "host.exec"));
        assert!(!sandbox.iter().any(|tool| tool.tool_id == "workspace.read"));

        let workspace = tool_bundle_workspace();
        assert!(
            workspace
                .iter()
                .any(|tool| tool.tool_id == "workspace.read")
        );
        assert!(!workspace.iter().any(|tool| tool.tool_id == "host.exec"));
    }

    #[test]
    fn registry_builder_merges_and_removes_tools() {
        let registry = crate::contracts::ToolRegistryBuilder::new()
            .with_bundle(tool_bundle_inspect())
            .with_bundle(tool_bundle_workspace())
            .without_tool("workspace.commit")
            .build()
            .expect("registry");

        assert!(registry.contains_key("introspect.manifest"));
        assert!(registry.contains_key("workspace.read"));
        assert!(!registry.contains_key("workspace.commit"));
    }

    #[test]
    fn profile_builder_validates_against_registry() {
        let registry = crate::contracts::ToolRegistryBuilder::new()
            .with_bundle(tool_bundle_inspect())
            .with_bundle(tool_bundle_host_sandbox())
            .without_tool("host.session.signal")
            .build()
            .expect("registry");

        let profile = crate::contracts::ToolProfileBuilder::new()
            .with_bundle(tool_bundle_inspect())
            .with_tool_id("host.exec")
            .without_tool("introspect.workflow_state")
            .build_for_registry(&registry)
            .expect("profile");

        assert_eq!(
            profile,
            vec!["introspect.manifest".to_string(), "host.exec".to_string()]
        );

        let err = crate::contracts::ToolProfileBuilder::new()
            .with_tool_id("missing.tool")
            .build_for_registry(&registry)
            .expect_err("unknown tool rejected");
        assert!(err.contains("missing.tool"));
    }
}
