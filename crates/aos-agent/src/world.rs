use alloc::string::String;
use alloc::vec::Vec;
use aos_wasm_sdk::{
    AirManifestParts, AirRoute, AirSchemaExport, AirWorkflowExport, air_manifest_json,
};

use crate::{
    ActiveToolBatch, EffectiveTool, EffectiveToolSet, HostCommand, HostSessionStatus,
    PendingBlobGet, PendingBlobGetKind, PendingBlobPut, PendingBlobPutKind, PendingFollowUpTurn,
    PlannedToolCall, ReasoningEffort, RunConfig, RunId, SessionConfig, SessionId, SessionIngress,
    SessionIngressKind, SessionLifecycle, SessionLifecycleChanged, SessionNoop, SessionState,
    SessionWorkflow, SessionWorkflowEvent, SharedPendingBlobGet, SharedPendingBlobPut,
    ToolAvailabilityRule, ToolBatchId, ToolBatchPlan, ToolCallLlmResult, ToolCallObserved,
    ToolCallStatus, ToolExecutionPlan, ToolExecutor, ToolMapper, ToolOverrideScope,
    ToolParallelismHint, ToolRuntimeContext, ToolSpec,
};

const MANIFEST_SCHEMAS: &[&str] = &[
    "aos.agent/SessionId@1",
    "aos.agent/RunId@1",
    "aos.agent/ToolBatchId@1",
    "aos.agent/ToolCallStatus@1",
    "aos.agent/ToolExecutor@1",
    "aos.agent/ToolAvailabilityRule@1",
    "aos.agent/ToolParallelismHint@1",
    "aos.agent/ToolSpec@1",
    "aos.agent/HostSessionStatus@1",
    "aos.agent/ToolRuntimeContext@1",
    "aos.agent/EffectiveTool@1",
    "aos.agent/EffectiveToolSet@1",
    "aos.agent/ToolCallObserved@1",
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
    "aos.agent/PendingBlobGetKind@1",
    "aos.agent/PendingBlobGet@1",
    "aos.agent/PendingBlobPutKind@1",
    "aos.agent/PendingBlobPut@1",
    "aos.agent/PendingFollowUpTurn@1",
    "aos.agent/SessionState@1",
    "aos.agent/SessionNoop@1",
    "aos.agent/SessionWorkflowEvent@1",
];

const MANIFEST_MODULES: &[&str] = &["aos.agent/SessionWorkflow_wasm@1"];
const MANIFEST_WORKFLOWS: &[&str] = &["aos.agent/SessionWorkflow@1"];
const MANIFEST_ROUTES: &[AirRoute] = &[
    AirRoute {
        event: "aos.agent/SessionIngress@1",
        workflow: "aos.agent/SessionWorkflow@1",
        key_field: "session_id",
    },
    AirRoute {
        event: "sys/WorkspaceCommit@1",
        workflow: "sys/Workspace@1",
        key_field: "workspace",
    },
];

pub fn aos_air_nodes() -> Vec<String> {
    let mut nodes = Vec::new();

    nodes.push(SessionId::air_schema_json());
    nodes.push(RunId::air_schema_json());
    nodes.push(ToolBatchId::air_schema_json());
    nodes.push(ToolCallStatus::air_schema_json());
    nodes.push(ToolMapper::air_schema_json());
    nodes.push(ToolExecutor::air_schema_json());
    nodes.push(ToolAvailabilityRule::air_schema_json());
    nodes.push(ToolParallelismHint::air_schema_json());
    nodes.push(ToolSpec::air_schema_json());
    nodes.push(HostSessionStatus::air_schema_json());
    nodes.push(ToolRuntimeContext::air_schema_json());
    nodes.push(EffectiveTool::air_schema_json());
    nodes.push(EffectiveToolSet::air_schema_json());
    nodes.push(ToolCallObserved::air_schema_json());
    nodes.push(ToolCallLlmResult::air_schema_json());
    nodes.push(PlannedToolCall::air_schema_json());
    nodes.push(ToolExecutionPlan::air_schema_json());
    nodes.push(ToolBatchPlan::air_schema_json());
    nodes.push(ActiveToolBatch::air_schema_json());
    nodes.push(ToolOverrideScope::air_schema_json());
    nodes.push(ReasoningEffort::air_schema_json());
    nodes.push(SessionConfig::air_schema_json());
    nodes.push(RunConfig::air_schema_json());
    nodes.push(SessionLifecycle::air_schema_json());
    nodes.push(HostCommand::air_schema_json());
    nodes.push(SessionIngressKind::air_schema_json());
    nodes.push(SessionIngress::air_schema_json());
    nodes.push(SessionLifecycleChanged::air_schema_json());
    nodes.push(PendingBlobGetKind::air_schema_json());
    nodes.push(PendingBlobGet::air_schema_json());
    nodes.push(PendingBlobPutKind::air_schema_json());
    nodes.push(PendingBlobPut::air_schema_json());
    nodes.push(PendingFollowUpTurn::air_schema_json());
    nodes.push(SharedPendingBlobGet::air_schema_json());
    nodes.push(SharedPendingBlobPut::air_schema_json());
    nodes.push(SessionState::air_schema_json());
    nodes.push(SessionWorkflowEvent::air_schema_json());
    nodes.push(SessionNoop::air_schema_json());

    nodes.push(<SessionWorkflow as AirWorkflowExport>::air_module_json());
    nodes.push(<SessionWorkflow as AirWorkflowExport>::air_workflow_json());
    nodes.push(air_manifest_json(AirManifestParts {
        air_version: "2",
        schemas: MANIFEST_SCHEMAS,
        modules: MANIFEST_MODULES,
        workflows: MANIFEST_WORKFLOWS,
        secrets: &[],
        effects: &[],
        routes: MANIFEST_ROUTES,
    }));

    nodes
}
