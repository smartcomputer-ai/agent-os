use crate::{
    SessionId, SessionState, SessionWorkflowEvent,
    helpers::{apply_session_workflow_event, emit_session_lifecycle_changed, map_reduce_error},
};
use aos_wasm_sdk::{ReduceError, Value, Workflow, WorkflowCtx};

#[derive(Default)]
#[aos_wasm_sdk::air_workflow(
    name = "aos.agent/SessionWorkflow@1",
    module = "aos.agent/SessionWorkflow_wasm@1",
    state = SessionState,
    event = SessionWorkflowEvent,
    context = aos_wasm_sdk::WorkflowContext,
    key_schema = SessionId,
    effects = [
        aos_wasm_sdk::BlobPutParams,
        aos_wasm_sdk::BlobGetParams,
        aos_wasm_sdk::LlmGenerateParams,
        aos_wasm_sdk::HostSessionOpenParams,
        aos_wasm_sdk::HostExecParams,
        aos_wasm_sdk::HostSessionSignalParams,
        aos_wasm_sdk::HostFsReadFileParams,
        aos_wasm_sdk::HostFsWriteFileParams,
        aos_wasm_sdk::HostFsEditFileParams,
        aos_wasm_sdk::HostFsApplyPatchParams,
        aos_wasm_sdk::HostFsGrepParams,
        aos_wasm_sdk::HostFsGlobParams,
        aos_wasm_sdk::HostFsStatParams,
        aos_wasm_sdk::HostFsExistsParams,
        aos_wasm_sdk::HostFsListDirParams,
        aos_wasm_sdk::IntrospectManifestParams,
        aos_wasm_sdk::IntrospectWorkflowStateParams,
        aos_wasm_sdk::IntrospectListCellsParams,
        aos_wasm_sdk::WorkspaceResolveParams,
        aos_wasm_sdk::WorkspaceEmptyRootParams,
        aos_wasm_sdk::WorkspaceListParams,
        aos_wasm_sdk::WorkspaceReadRefParams,
        aos_wasm_sdk::WorkspaceReadBytesParams,
        aos_wasm_sdk::WorkspaceWriteBytesParams,
        aos_wasm_sdk::WorkspaceWriteRefParams,
        aos_wasm_sdk::WorkspaceRemoveParams,
        aos_wasm_sdk::WorkspaceDiffParams,
    ]
)]
pub struct SessionWorkflow;

impl Workflow for SessionWorkflow {
    type State = SessionState;
    type Event = SessionWorkflowEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        let prev_lifecycle = ctx.state.lifecycle;
        let prev_run_id = ctx.state.active_run_id.clone();
        let out = apply_session_workflow_event(&mut ctx.state, &event).map_err(map_reduce_error)?;
        for domain_event in out.domain_events {
            domain_event.emit(ctx);
        }
        for effect in out.effects {
            effect.emit(ctx);
        }
        emit_session_lifecycle_changed(ctx, prev_lifecycle, prev_run_id);
        Ok(())
    }
}
