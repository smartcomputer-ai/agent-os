pub mod adapters;
pub mod config;

use std::sync::Arc;

use adapters::blob_get::BlobGetAdapter;
use adapters::blob_put::BlobPutAdapter;
use adapters::host::{make_fabric_host_adapter_set, make_host_adapter_set};
use adapters::registry::AdapterRegistry;
#[cfg(not(feature = "adapter-http"))]
use adapters::stub::StubHttpAdapter;
use adapters::stub::{
    StubLlmAdapter, StubTimerAdapter, StubVaultPutAdapter, StubVaultRotateAdapter,
};
use aos_kernel::Store;
use config::EffectAdapterConfig;

pub use adapters::registry;
pub use adapters::traits;
pub use adapters::traits::AsyncEffectAdapter;

pub fn default_registry<S: Store + 'static>(
    store: Arc<S>,
    config: &EffectAdapterConfig,
) -> AdapterRegistry {
    let mut registry = AdapterRegistry::new();
    registry.register(Box::new(StubTimerAdapter));
    registry.register(Box::new(StubVaultPutAdapter));
    registry.register(Box::new(StubVaultRotateAdapter));
    registry.register(Box::new(BlobPutAdapter::new(store.clone())));
    registry.register(Box::new(BlobGetAdapter::new(store.clone())));

    let host_adapters = make_host_adapter_set(store.clone());
    registry.register(Box::new(host_adapters.session_open));
    registry.register(Box::new(host_adapters.exec));
    registry.register(Box::new(host_adapters.session_signal));
    registry.register(Box::new(host_adapters.fs_read_file));
    registry.register(Box::new(host_adapters.fs_write_file));
    registry.register(Box::new(host_adapters.fs_edit_file));
    registry.register(Box::new(host_adapters.fs_apply_patch));
    registry.register(Box::new(host_adapters.fs_grep));
    registry.register(Box::new(host_adapters.fs_glob));
    registry.register(Box::new(host_adapters.fs_stat));
    registry.register(Box::new(host_adapters.fs_exists));
    registry.register(Box::new(host_adapters.fs_list_dir));

    if let Some(fabric_cfg) = &config.fabric {
        let fabric_adapters = make_fabric_host_adapter_set(store.clone(), fabric_cfg.clone());
        registry.register(Box::new(fabric_adapters.session_open));
        registry.register(Box::new(fabric_adapters.exec));
        registry.register(Box::new(fabric_adapters.session_signal));
        registry.register(Box::new(fabric_adapters.fs_read_file));
        registry.register(Box::new(fabric_adapters.fs_write_file));
        registry.register(Box::new(fabric_adapters.fs_edit_file));
        registry.register(Box::new(fabric_adapters.fs_apply_patch));
        registry.register(Box::new(fabric_adapters.fs_grep));
        registry.register(Box::new(fabric_adapters.fs_glob));
        registry.register(Box::new(fabric_adapters.fs_stat));
        registry.register(Box::new(fabric_adapters.fs_exists));
        registry.register(Box::new(fabric_adapters.fs_list_dir));
    }

    #[cfg(feature = "adapter-http")]
    {
        registry.register(Box::new(adapters::http::HttpAdapter::new(
            store.clone(),
            config.http.clone(),
        )));
    }
    #[cfg(not(feature = "adapter-http"))]
    {
        registry.register(Box::new(StubHttpAdapter));
    }

    #[cfg(feature = "adapter-llm")]
    {
        if let Some(llm_cfg) = &config.llm {
            registry.register(Box::new(adapters::llm::LlmAdapter::new(
                store,
                llm_cfg.clone(),
            )));
        } else {
            registry.register(Box::new(StubLlmAdapter));
        }
    }
    #[cfg(not(feature = "adapter-llm"))]
    {
        registry.register(Box::new(StubLlmAdapter));
    }

    for (route_id, provider) in &config.adapter_routes {
        if !registry.register_route(route_id.as_str(), provider.adapter_kind.as_str()) {
            log::warn!(
                "host profile route '{}' targets unknown adapter kind '{}'",
                route_id,
                provider.adapter_kind
            );
        }
    }

    registry
}
