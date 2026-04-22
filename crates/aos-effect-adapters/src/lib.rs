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
use aos_effects::effect_ops;
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

    let has_fabric = config.fabric.is_some();
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

    register_builtin_route_aliases(&mut registry);
    if has_fabric {
        register_fabric_host_route_overrides(&mut registry);
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

fn register_builtin_route_aliases(registry: &mut AdapterRegistry) {
    for (entrypoint, canonical) in [
        ("http.request", effect_ops::HTTP_REQUEST),
        ("blob.put", effect_ops::BLOB_PUT),
        ("blob.get", effect_ops::BLOB_GET),
        ("timer.set", effect_ops::TIMER_SET),
        ("portal.send", effect_ops::PORTAL_SEND),
        ("host.session.open", effect_ops::HOST_SESSION_OPEN),
        ("host.exec", effect_ops::HOST_EXEC),
        ("host.session.signal", effect_ops::HOST_SESSION_SIGNAL),
        ("host.fs.read_file", effect_ops::HOST_FS_READ_FILE),
        ("host.fs.write_file", effect_ops::HOST_FS_WRITE_FILE),
        ("host.fs.edit_file", effect_ops::HOST_FS_EDIT_FILE),
        ("host.fs.apply_patch", effect_ops::HOST_FS_APPLY_PATCH),
        ("host.fs.grep", effect_ops::HOST_FS_GREP),
        ("host.fs.glob", effect_ops::HOST_FS_GLOB),
        ("host.fs.stat", effect_ops::HOST_FS_STAT),
        ("host.fs.exists", effect_ops::HOST_FS_EXISTS),
        ("host.fs.list_dir", effect_ops::HOST_FS_LIST_DIR),
        ("llm.generate", effect_ops::LLM_GENERATE),
        ("vault.put", effect_ops::VAULT_PUT),
        ("vault.rotate", effect_ops::VAULT_ROTATE),
    ] {
        register_route_alias_pair(registry, entrypoint, canonical);
    }
}

fn register_fabric_host_route_overrides(registry: &mut AdapterRegistry) {
    for (entrypoint, canonical, fabric) in [
        (
            "host.session.open",
            effect_ops::HOST_SESSION_OPEN,
            "host.session.open.fabric",
        ),
        ("host.exec", effect_ops::HOST_EXEC, "host.exec.fabric"),
        (
            "host.session.signal",
            effect_ops::HOST_SESSION_SIGNAL,
            "host.session.signal.fabric",
        ),
        (
            "host.fs.read_file",
            effect_ops::HOST_FS_READ_FILE,
            "host.fs.read_file.fabric",
        ),
        (
            "host.fs.write_file",
            effect_ops::HOST_FS_WRITE_FILE,
            "host.fs.write_file.fabric",
        ),
        (
            "host.fs.edit_file",
            effect_ops::HOST_FS_EDIT_FILE,
            "host.fs.edit_file.fabric",
        ),
        (
            "host.fs.apply_patch",
            effect_ops::HOST_FS_APPLY_PATCH,
            "host.fs.apply_patch.fabric",
        ),
        (
            "host.fs.grep",
            effect_ops::HOST_FS_GREP,
            "host.fs.grep.fabric",
        ),
        (
            "host.fs.glob",
            effect_ops::HOST_FS_GLOB,
            "host.fs.glob.fabric",
        ),
        (
            "host.fs.stat",
            effect_ops::HOST_FS_STAT,
            "host.fs.stat.fabric",
        ),
        (
            "host.fs.exists",
            effect_ops::HOST_FS_EXISTS,
            "host.fs.exists.fabric",
        ),
        (
            "host.fs.list_dir",
            effect_ops::HOST_FS_LIST_DIR,
            "host.fs.list_dir.fabric",
        ),
    ] {
        let _ = registry.register_route(entrypoint, fabric);
        let _ = registry.register_route(canonical, fabric);
    }
}

fn register_route_alias_pair(registry: &mut AdapterRegistry, entrypoint: &str, canonical: &str) {
    let entrypoint_known = registry.has_route(entrypoint);
    let canonical_known = registry.has_route(canonical);
    if canonical_known {
        let _ = registry.register_route(entrypoint, canonical);
    }
    if entrypoint_known {
        let _ = registry.register_route(canonical, entrypoint);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aos_kernel::MemStore;

    use super::*;

    #[test]
    fn default_registry_exposes_builtin_entrypoint_and_canonical_routes() {
        let store = Arc::new(MemStore::default());
        let mut config = EffectAdapterConfig::default();
        config.llm = None;

        let registry = default_registry(store, &config);

        assert!(registry.has_route("llm.generate"));
        assert!(registry.has_route(effect_ops::LLM_GENERATE));
        assert!(registry.has_route("llm.default"));
        assert!(registry.has_route("host.exec"));
        assert!(registry.has_route(effect_ops::HOST_EXEC));
        assert!(registry.has_route("host.exec.default"));
    }

    #[test]
    fn default_registry_routes_builtin_host_entrypoints_to_fabric_when_configured() {
        let store = Arc::new(MemStore::default());
        let mut config = EffectAdapterConfig::default();
        config.fabric = Some(crate::config::FabricAdapterConfig {
            controller_url: "http://127.0.0.1:1".into(),
            bearer_token: None,
            request_timeout: std::time::Duration::from_secs(1),
            exec_progress_interval: std::time::Duration::from_secs(1),
            default_image: None,
            default_runtime_class: None,
            default_network_mode: None,
        });

        let registry = default_registry(store, &config);
        let mappings = registry.route_mappings();

        assert_eq!(
            mappings.get("host.exec").map(String::as_str),
            Some("host.exec.fabric")
        );
        assert_eq!(
            mappings.get(effect_ops::HOST_EXEC).map(String::as_str),
            Some("host.exec.fabric")
        );
        assert_eq!(
            mappings.get("host.exec.default").map(String::as_str),
            Some("host.exec.fabric")
        );
    }
}
