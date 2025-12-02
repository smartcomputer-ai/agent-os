use thiserror::Error;

#[derive(Debug, Error)]
pub enum HostError {
    #[error("kernel error: {0}")]
    Kernel(#[from] aos_kernel::KernelError),
    #[error("adapter error: {0}")]
    Adapter(String),
    #[error("invalid external event: {0}")]
    External(String),
}
