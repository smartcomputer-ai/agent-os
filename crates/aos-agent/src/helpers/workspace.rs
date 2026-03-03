use crate::contracts::WorkspaceSnapshot;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceStepInputs {
    pub message_refs: Vec<alloc::string::String>,
}

/// Build LLM message refs from active workspace snapshot plus per-step prompt refs.
///
/// - `history_message_refs` should be ordered oldest -> newest.
/// - `explicit_prompt_refs`, when provided, override workspace prompt-pack refs.
pub fn materialize_workspace_step_inputs(
    active_snapshot: Option<&WorkspaceSnapshot>,
    history_message_refs: Vec<alloc::string::String>,
    explicit_prompt_refs: Option<Vec<alloc::string::String>>,
) -> WorkspaceStepInputs {
    let mut message_refs = if let Some(explicit) = explicit_prompt_refs {
        explicit
    } else if let Some(reference) = active_snapshot.and_then(|snap| snap.prompt_pack_ref.clone()) {
        vec![reference]
    } else {
        Vec::new()
    };
    message_refs.extend(history_message_refs);

    WorkspaceStepInputs { message_refs }
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
            prompt_pack_ref: Some(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
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
    fn explicit_prompt_refs_override_workspace_prompt_pack_ref() {
        let inputs = materialize_workspace_step_inputs(
            Some(&snapshot()),
            vec!["sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".into()],
            Some(vec![
                "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".into(),
            ]),
        );
        assert_eq!(
            inputs.message_refs,
            vec![
                alloc::string::String::from(
                    "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                ),
                alloc::string::String::from(
                    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                ),
            ]
        );
    }
}
