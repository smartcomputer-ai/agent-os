use crate::bootstrap::build_control_deps_from_worker_runtime;
use crate::control::{ControlError, ControlFacade};
use crate::worker::HostedWorkerRuntime;

pub fn control_facade_from_worker_runtime(
    runtime: HostedWorkerRuntime,
) -> Result<ControlFacade, ControlError> {
    ControlFacade::new(build_control_deps_from_worker_runtime(runtime)?)
}
