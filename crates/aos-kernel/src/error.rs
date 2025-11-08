use thiserror::Error;

#[derive(Debug, Error)]
pub enum KernelError {
    #[error("store error: {0}")]
    Store(#[from] aos_store::StoreError),
    #[error("wasm runtime error: {0}")]
    Wasm(#[from] anyhow::Error),
    #[error("manifest loader error: {0}")]
    Manifest(String),
    #[error("missing reducer '{0}'")]
    ReducerNotFound(String),
    #[error("invalid reducer output: {0}")]
    ReducerOutput(String),
    #[error("effect manager error: {0}")]
    EffectManager(String),
    #[error("unknown effect receipt for {0}")]
    UnknownReceipt(String),
    #[error("failed to decode receipt payload: {0}")]
    ReceiptDecode(String),
    #[error("unsupported reducer receipt kind '{0}'")]
    UnsupportedReducerReceipt(String),
}
