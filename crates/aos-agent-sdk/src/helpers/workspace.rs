use crate::contracts::WorkspaceSnapshot;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceStepInputs {
    pub message_refs: Vec<alloc::string::String>,
    pub tool_refs: Option<Vec<alloc::string::String>>,
}

/// Build LLM step refs from active workspace snapshot plus per-step inputs.
///
/// - `history_message_refs` should be ordered oldest -> newest.
/// - `explicit_tool_refs`, when provided, override workspace tool catalog refs.
pub fn materialize_workspace_step_inputs(
    active_snapshot: Option<&WorkspaceSnapshot>,
    history_message_refs: Vec<alloc::string::String>,
    explicit_tool_refs: Option<Vec<alloc::string::String>>,
) -> WorkspaceStepInputs {
    let mut message_refs = Vec::new();
    if let Some(reference) = active_snapshot.and_then(|snap| snap.prompt_pack_ref.clone()) {
        message_refs.push(reference);
    }
    message_refs.extend(history_message_refs);

    let tool_refs = if explicit_tool_refs.is_some() {
        explicit_tool_refs
    } else {
        active_snapshot
            .and_then(|snap| snap.tool_catalog_ref.clone())
            .map(|reference| vec![reference])
    };

    WorkspaceStepInputs {
        message_refs,
        tool_refs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn snapshot() -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            workspace: "agent".into(),
            version: Some(1),
            root_hash: None,
            index_ref: None,
            prompt_pack: Some("default".into()),
            tool_catalog: Some("default".into()),
            prompt_pack_ref: Some(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            ),
            tool_catalog_ref: Some(
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            ),
        }
    }

    #[test]
    fn prepends_prompt_pack_ref_to_history() {
        let inputs = materialize_workspace_step_inputs(
            Some(&snapshot()),
            vec!["sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".into()],
            None,
        );
        assert_eq!(inputs.message_refs.len(), 2);
        assert_eq!(
            inputs.message_refs[0],
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(
            inputs.message_refs[1],
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        );
    }

    #[test]
    fn uses_workspace_tool_catalog_ref_when_no_explicit_tool_refs() {
        let inputs = materialize_workspace_step_inputs(Some(&snapshot()), vec![], None);
        assert_eq!(
            inputs.tool_refs,
            Some(vec![
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into()
            ])
        );
    }

    #[test]
    fn explicit_tool_refs_override_workspace_catalog_ref() {
        let inputs = materialize_workspace_step_inputs(
            Some(&snapshot()),
            vec![],
            Some(vec![
                "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".into(),
            ]),
        );
        assert_eq!(
            inputs.tool_refs,
            Some(vec![
                "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".into()
            ])
        );
    }
}
