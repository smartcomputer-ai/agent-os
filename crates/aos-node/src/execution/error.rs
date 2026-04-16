use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("kernel error: {0}")]
    Kernel(#[from] aos_kernel::KernelError),
    #[error("adapter error: {0}")]
    Adapter(String),
    #[error("runtime error: {0}")]
    External(String),
    #[error("manifest error: {0}")]
    Manifest(String),
    #[error("route error: {0}")]
    Route(String),
    #[error("timer error: {0}")]
    Timer(String),
    #[error("execution error: {0}")]
    Execution(String),
    #[error("invalid execution class: {0}")]
    ExecutionClass(String),
}
