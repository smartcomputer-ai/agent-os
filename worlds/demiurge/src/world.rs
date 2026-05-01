use crate::workflow::{
    Demiurge, DemiurgeState, DemiurgeWorkflowEvent, PendingStage, TaskConfig, TaskFailure,
    TaskFinished, TaskStatus, TaskSubmitted,
};
use aos_agent::{
    ReasoningEffort, RunId, RunLifecycleChanged, SessionId, SessionInput,
    SessionLifecycleChanged, SessionNoop, SessionState, SessionStatusChanged, SessionWorkflow,
    SessionWorkflowEvent,
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
        import_schema_types: [
            SessionId,
            RunId,
            ReasoningEffort,
            SessionInput,
            SessionLifecycleChanged,
            SessionStatusChanged,
            RunLifecycleChanged,
            SessionNoop,
            SessionState,
            SessionWorkflowEvent,
        ],
        import_workflow_types: [
            SessionWorkflow,
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
                event: "aos.agent/SessionInput@1",
                workflow: SessionWorkflow,
                key_field: "session_id",
            },
        ],
    }
}
