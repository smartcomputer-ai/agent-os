use crate::{
    ActiveToolBatch, EffectiveTool, EffectiveToolSet, HostCommand, HostMountConfig,
    HostSessionOpenConfig, HostSessionStatus, HostTargetConfig, PendingBlobGet, PendingBlobGetKind,
    PendingBlobPut, PendingBlobPutKind, PendingFollowUpTurn, PlannedToolCall, ReasoningEffort,
    RunConfig, RunId, SessionConfig, SessionId, SessionIngress, SessionIngressKind,
    SessionLifecycle, SessionLifecycleChanged, SessionNoop, SessionState, SessionWorkflow,
    SessionWorkflowEvent, SharedPendingBlobGet, SharedPendingBlobPut, ToolAvailabilityRule,
    ToolBatchId, ToolBatchPlan, ToolCallLlmResult, ToolCallObserved, ToolCallStatus,
    ToolExecutionPlan, ToolExecutor, ToolMapper, ToolOverrideScope, ToolParallelismHint,
    ToolRuntimeContext, ToolSpec,
};

aos_wasm_sdk::aos_air_world! {
    pub fn aos_air_nodes() {
        air_version: "2",
        schemas: [
            SessionId,
            RunId,
            ToolBatchId,
            ToolCallStatus,
            ToolExecutor,
            ToolAvailabilityRule,
            ToolParallelismHint,
            ToolSpec,
            ToolMapper,
            HostSessionStatus,
            ToolRuntimeContext,
            EffectiveTool,
            EffectiveToolSet,
            ToolCallObserved,
            ToolCallLlmResult,
            PlannedToolCall,
            ToolExecutionPlan,
            ToolBatchPlan,
            ActiveToolBatch,
            ToolOverrideScope,
            ReasoningEffort,
            HostMountConfig,
            HostTargetConfig,
            HostSessionOpenConfig,
            SessionConfig,
            RunConfig,
            SessionLifecycle,
            HostCommand,
            SessionIngressKind,
            SessionIngress,
            SessionLifecycleChanged,
            PendingBlobGetKind,
            PendingBlobGet,
            PendingBlobPutKind,
            PendingBlobPut,
            PendingFollowUpTurn,
            SharedPendingBlobGet,
            SharedPendingBlobPut,
            SessionState,
            SessionNoop,
            SessionWorkflowEvent,
        ],
        workflows: [SessionWorkflow],
        routing: [
            {
                event_schema: SessionIngress,
                workflow: SessionWorkflow,
                key_field: "session_id",
            },
            {
                event: "sys/WorkspaceCommit@1",
                workflow_name: "sys/Workspace@1",
                key_field: "workspace",
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::string::String;
    use std::vec::Vec;

    use super::aos_air_nodes;

    #[test]
    fn generated_air_matches_checked_in_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nodes = aos_air_nodes();
        let fragments: Vec<&str> = nodes.iter().map(String::as_str).collect();
        aos_authoring::write_generated_air_nodes(temp.path(), &fragments)
            .expect("write generated AIR");

        let expected_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("air/generated");
        let actual_dir = temp.path().join("air/generated");
        assert_eq!(
            generated_file_names(&actual_dir),
            generated_file_names(&expected_dir)
        );

        for name in generated_file_names(&expected_dir) {
            let actual = fs::read(actual_dir.join(&name)).expect("read generated file");
            let expected = fs::read(expected_dir.join(&name)).expect("read checked-in file");
            assert_eq!(
                String::from_utf8(actual).expect("generated file utf8"),
                String::from_utf8(expected).expect("checked-in file utf8"),
                "checked-in generated AIR is stale: {}",
                name.display()
            );
        }
    }

    fn generated_file_names(dir: &Path) -> Vec<PathBuf> {
        let mut names = fs::read_dir(dir)
            .expect("read generated dir")
            .map(|entry| entry.expect("read entry").path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
            .map(|path| {
                path.file_name()
                    .map(PathBuf::from)
                    .expect("generated file name")
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }
}
