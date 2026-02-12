//! Provider adapter contract.
//!
//! Implemented in P05+.

use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;

use crate::errors::SDKError;
use crate::stream::StreamEventStream;
use crate::types::{Request, Response};

/// Provider adapter contract.
#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn name(&self) -> &str;

    async fn complete(&self, request: Request) -> Result<Response, SDKError>;

    async fn stream(&self, request: Request) -> Result<StreamEventStream, SDKError>;

    /// Optional lifecycle hook called by `Client` when registering providers.
    fn initialize(&self) -> Result<(), SDKError> {
        Ok(())
    }

    /// Optional lifecycle hook called by `Client::close()`.
    fn close(&self) -> Result<(), SDKError> {
        Ok(())
    }

    /// Optional capability hook for tool choice support.
    fn supports_tool_choice(&self, _mode: &str) -> bool {
        false
    }
}

/// Factory for building adapters from environment variables.
pub trait ProviderFactory: Send + Sync {
    fn provider_id(&self) -> &'static str;
    fn from_env(&self) -> Option<Arc<dyn ProviderAdapter>>;
}

static PROVIDER_FACTORIES: OnceLock<Mutex<Vec<Arc<dyn ProviderFactory>>>> = OnceLock::new();

fn factories() -> &'static Mutex<Vec<Arc<dyn ProviderFactory>>> {
    PROVIDER_FACTORIES.get_or_init(|| Mutex::new(Vec::new()))
}

/// Register a provider factory for Client::from_env().
///
/// Provider adapter crates should call this during initialization.
pub fn register_provider_factory(factory: Arc<dyn ProviderFactory>) {
    let mut registry = factories().lock().expect("provider factory registry");
    if let Some(index) = registry
        .iter()
        .position(|existing| existing.provider_id() == factory.provider_id())
    {
        registry[index] = factory;
    } else {
        registry.push(factory);
    }
}

/// Get a snapshot of registered factories.
pub fn registered_factories() -> Vec<Arc<dyn ProviderFactory>> {
    let registry = factories().lock().expect("provider factory registry");
    registry.clone()
}

#[cfg(test)]
pub(crate) fn clear_provider_factories_for_tests() {
    let mut registry = factories().lock().expect("provider factory registry");
    registry.clear();
}
