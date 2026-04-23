use aos_effect_types::{WorkspaceCommit, WorkspaceHistory, WorkspaceVersion};
use aos_wasm_abi::{ABI_VERSION, WorkflowInput, WorkflowOutput};
use std::string::String;

use crate::error::KernelError;

pub(super) fn invoke_builtin_workflow(
    module_name: &str,
    entrypoint: &str,
    input: &WorkflowInput,
) -> Result<WorkflowOutput, KernelError> {
    match (module_name, entrypoint) {
        ("sys/builtin_workspaces@1", "step") => invoke_workspace(input),
        _ => Err(KernelError::Manifest(format!(
            "builtin workflow module '{module_name}' entrypoint '{entrypoint}' is not implemented"
        ))),
    }
}

fn invoke_workspace(input: &WorkflowInput) -> Result<WorkflowOutput, KernelError> {
    if input.version != ABI_VERSION {
        return Err(KernelError::WorkflowOutput(format!(
            "unsupported workflow ABI version {}",
            input.version
        )));
    }

    let event: WorkspaceCommit = serde_cbor::from_slice(&input.event.value).map_err(|err| {
        KernelError::WorkflowOutput(format!("decode sys/Workspace@1 event: {err}"))
    })?;
    let mut state: WorkspaceHistory = match input.state.as_deref() {
        Some(bytes) => serde_cbor::from_slice(bytes).map_err(|err| {
            KernelError::WorkflowOutput(format!("decode sys/Workspace@1 state: {err}"))
        })?,
        None => WorkspaceHistory::default(),
    };

    reduce_workspace(event, input.event.key.as_deref(), &mut state)?;

    let state = serde_cbor::to_vec(&state).map_err(|err| {
        KernelError::WorkflowOutput(format!("encode sys/Workspace@1 state: {err}"))
    })?;
    Ok(WorkflowOutput {
        state: Some(state),
        ..WorkflowOutput::default()
    })
}

fn reduce_workspace(
    event: WorkspaceCommit,
    key: Option<&[u8]>,
    state: &mut WorkspaceHistory,
) -> Result<(), KernelError> {
    let workspace = event.workspace;
    if !is_valid_workspace_name(&workspace) {
        return Err(KernelError::WorkflowOutput(
            "invalid workspace name".to_string(),
        ));
    }

    if let Some(key) = key {
        let decoded_key: String = if let Ok(decoded) = serde_cbor::from_slice(key) {
            decoded
        } else {
            String::from_utf8(key.to_vec()).map_err(|_| {
                KernelError::WorkflowOutput("workspace key decode failed".to_string())
            })?
        };
        if decoded_key != workspace {
            return Err(KernelError::WorkflowOutput(
                "workspace key mismatch".to_string(),
            ));
        }
    }

    if let Some(expected) = event.expected_head
        && expected != state.latest
    {
        return Err(KernelError::WorkflowOutput(
            "workspace head mismatch".to_string(),
        ));
    }

    let next: WorkspaceVersion = state.latest.saturating_add(1);
    state.latest = next;
    state.versions.insert(next, event.meta);
    Ok(())
}

fn is_valid_workspace_name(name: &str) -> bool {
    if name.is_empty() || name.contains('/') {
        return false;
    }
    name.chars().all(is_url_safe_char)
}

fn is_url_safe_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '~' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_effect_types::{HashRef, WorkspaceCommitMeta};

    fn hash_ref(byte: char) -> HashRef {
        HashRef::new(format!("sha256:{}", byte.to_string().repeat(64))).expect("hash ref")
    }

    fn commit(workspace: &str, expected_head: Option<u64>) -> WorkspaceCommit {
        WorkspaceCommit {
            workspace: workspace.to_string(),
            expected_head,
            meta: WorkspaceCommitMeta {
                root_hash: hash_ref('a'),
                owner: "tester".into(),
                created_at: 1234,
            },
        }
    }

    fn key(workspace: &str) -> Vec<u8> {
        aos_cbor::to_canonical_cbor(&workspace.to_string()).expect("encode key")
    }

    #[test]
    fn workspace_first_commit_creates_version_one() {
        let mut state = WorkspaceHistory::default();

        reduce_workspace(commit("main", None), Some(&key("main")), &mut state).expect("reduce");

        assert_eq!(state.latest, 1);
        assert!(state.versions.contains_key(&1));
    }

    #[test]
    fn workspace_expected_head_mismatch_fails() {
        let mut state = WorkspaceHistory::default();

        let err =
            reduce_workspace(commit("main", Some(1)), Some(&key("main")), &mut state).unwrap_err();

        assert!(matches!(err, KernelError::WorkflowOutput(_)));
        assert_eq!(state.latest, 0);
    }

    #[test]
    fn workspace_invalid_name_fails() {
        let mut state = WorkspaceHistory::default();

        let err = reduce_workspace(commit("bad/name", None), Some(&key("bad/name")), &mut state)
            .unwrap_err();

        assert!(matches!(err, KernelError::WorkflowOutput(_)));
        assert_eq!(state.latest, 0);
    }

    #[test]
    fn workspace_key_mismatch_fails() {
        let mut state = WorkspaceHistory::default();

        let err =
            reduce_workspace(commit("main", None), Some(&key("other")), &mut state).unwrap_err();

        assert!(matches!(err, KernelError::WorkflowOutput(_)));
        assert_eq!(state.latest, 0);
    }
}
