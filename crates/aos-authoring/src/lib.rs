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
    build_bundle_from_local_world_with_profile, build_loaded_manifest_from_air_sources,
    build_loaded_manifest_from_authored_paths, compile_workflow,
    materialize_discovered_cargo_modules, resolve_placeholder_modules,
    resolve_sys_module_wasm_hash,
};
pub use bundle::{WorldBundle, build_patch_document, load_air_bundle};
pub use generated::{
    DEFAULT_AIR_EXPORT_BIN, GENERATED_AIR_DIR, write_generated_air_export_json,
    write_generated_air_from_cargo_export, write_generated_air_nodes,
};
pub use local::{local_kernel_config, local_state_paths, reset_local_runtime_state};
pub use manifest_loader::{
    AirSource, LoadedAssets, ZERO_HASH_SENTINEL, load_from_air_sources,
    load_from_air_sources_with_defs, load_from_assets, load_from_assets_with_defs,
    load_from_assets_with_imports, load_from_assets_with_imports_and_defs,
    parse_air_nodes_from_str,
};
pub use sync::{
    ResolvedAirPackage, ResolvedAirSources, ResolvedSecretValue, WorldConfig,
    load_all_world_secret_values, load_required_secret_value_map, load_world_config,
    resolve_world_air_sources,
};
pub use util::{has_placeholder_modules, is_placeholder_hash, patch_modules};
