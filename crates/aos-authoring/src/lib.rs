//! Shared authored-world build/upload helpers for node-facing CLIs and probes.

pub mod build;
pub mod bundle;
pub mod generated;
pub mod local;
pub mod manifest_loader;
pub mod sync;
pub mod util;

pub use build::{
    CompiledWorkflow, WorkflowBuildProfile, build_bundle_from_local_world,
    build_bundle_from_local_world_ephemeral, build_bundle_from_local_world_ephemeral_with_profile,
    build_bundle_from_local_world_with_profile, build_loaded_manifest_from_authored_paths,
    compile_workflow, materialize_imported_cargo_modules, resolve_placeholder_modules,
    resolve_sys_module_wasm_hash,
};
pub use bundle::{WorldBundle, build_patch_document, load_air_bundle};
pub use generated::{GENERATED_AIR_DIR, write_generated_air_nodes};
pub use local::{local_kernel_config, local_state_paths, reset_local_runtime_state};
pub use manifest_loader::{
    LoadedAssets, ZERO_HASH_SENTINEL, load_from_assets, load_from_assets_with_defs,
    load_from_assets_with_imports, load_from_assets_with_imports_and_defs,
    parse_air_nodes_from_str,
};
pub use sync::{
    ResolvedAirImport, ResolvedAirSources, ResolvedSecretValue, SyncConfig,
    load_all_sync_secret_values, load_required_secret_value_map, load_sync_config,
    resolve_air_sources,
};
pub use util::{has_placeholder_modules, is_placeholder_hash, patch_modules};
