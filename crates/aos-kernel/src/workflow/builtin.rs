use aos_wasm_abi::{WorkflowInput, WorkflowOutput};

use crate::error::KernelError;

pub(super) fn invoke_builtin_workflow(
    module_name: &str,
    entrypoint: &str,
    _input: &WorkflowInput,
) -> Result<WorkflowOutput, KernelError> {
    Err(KernelError::Manifest(format!(
        "builtin workflow module '{module_name}' entrypoint '{entrypoint}' is not implemented"
    )))
}
