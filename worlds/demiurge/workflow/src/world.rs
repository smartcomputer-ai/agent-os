use crate::workflow::{
    Demiurge, DemiurgeState, DemiurgeWorkflowEvent, PendingStage, TaskConfig, TaskFailure,
    TaskFinished, TaskStatus, TaskSubmitted,
};

aos_wasm_sdk::aos_air_secret! {
    pub(crate) struct OpenAiApiSecret {
        name: "llm/openai_api@1",
        binding_id: "llm/openai_api",
    }
}

aos_wasm_sdk::aos_air_secret! {
    pub(crate) struct AnthropicApiSecret {
        name: "llm/anthropic_api@1",
        binding_id: "llm/anthropic_api",
    }
}

aos_wasm_sdk::aos_air_world! {
    pub fn aos_air_nodes() {
        air_version: "2",
        schemas: [
            TaskConfig,
            TaskSubmitted,
            TaskStatus,
            TaskFailure,
            PendingStage,
            TaskFinished,
            DemiurgeState,
            DemiurgeWorkflowEvent,
        ],
        workflows: [Demiurge],
        secrets: [
            OpenAiApiSecret,
            AnthropicApiSecret,
        ],
        import_schemas: [
            "aos.agent/SessionId@1",
            "aos.agent/RunId@1",
            "aos.agent/ToolBatchId@1",
            "aos.agent/ToolCallStatus@1",
            "aos.agent/ToolExecutor@1",
            "aos.agent/ToolAvailabilityRule@1",
            "aos.agent/ToolParallelismHint@1",
            "aos.agent/ToolSpec@1",
            "aos.agent/ToolMapper@1",
            "aos.agent/HostSessionStatus@1",
            "aos.agent/ToolRuntimeContext@1",
            "aos.agent/EffectiveTool@1",
            "aos.agent/EffectiveToolSet@1",
            "aos.agent/ToolCallObserved@1",
            "aos.agent/ToolCallLlmResult@1",
            "aos.agent/PlannedToolCall@1",
            "aos.agent/ToolExecutionPlan@1",
            "aos.agent/ToolBatchPlan@1",
            "aos.agent/ActiveToolBatch@1",
            "aos.agent/ToolOverrideScope@1",
            "aos.agent/ReasoningEffort@1",
            "aos.agent/SessionConfig@1",
            "aos.agent/RunConfig@1",
            "aos.agent/SessionLifecycle@1",
            "aos.agent/HostCommand@1",
            "aos.agent/SessionIngressKind@1",
            "aos.agent/SessionIngress@1",
            "aos.agent/SessionLifecycleChanged@1",
            "aos.agent/PendingBlobPutKind@1",
            "aos.agent/PendingBlobPut@1",
            "aos.agent/PendingBlobGetKind@1",
            "aos.agent/PendingBlobGet@1",
            "aos.agent/PendingFollowUpTurn@1",
            "aos.agent/SessionState@1",
            "aos.agent/SessionNoop@1",
            "aos.agent/SessionWorkflowEvent@1",
        ],
        import_modules: [
            "aos.agent/SessionWorkflow_wasm@1",
        ],
        import_workflows: [
            "aos.agent/SessionWorkflow@1",
        ],
        routing: [
            {
                event_schema: TaskSubmitted,
                workflow: Demiurge,
                key_field: "task_id",
            },
            {
                event: "aos.agent/SessionLifecycleChanged@1",
                workflow: Demiurge,
                key_field: "session_id",
            },
            {
                event: "aos.agent/SessionIngress@1",
                workflow_name: "aos.agent/SessionWorkflow@1",
                key_field: "session_id",
            },
        ],
    }
}
