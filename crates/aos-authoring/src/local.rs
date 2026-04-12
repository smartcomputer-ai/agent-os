use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_kernel::{KernelConfig, LoadedManifest, MapSecretResolver, MemStore, SharedSecretResolver};
use aos_node::{EmbeddedWorldHarness, WorldId};
use aos_node::{FsCas, LocalStatePaths};
use aos_runtime::{EffectMode, HarnessBuilder, WorkflowHarness, WorldConfig};
use uuid::Uuid;

use crate::build::{
    WorkflowBuildProfile, build_bundle_from_local_world, build_loaded_manifest_from_authored_paths,
};
use crate::bundle::{WorldBundle, import_genesis};
use crate::sync::load_available_secret_value_map;

pub struct SeededLocalHarness {
    pub world_id: WorldId,
    pub harness: EmbeddedWorldHarness,
    pub store: Arc<FsCas>,
    pub warnings: Vec<String>,
}

pub fn local_state_paths(world_root: &Path) -> LocalStatePaths {
    LocalStatePaths::from_world_root(world_root)
}

pub fn reset_local_runtime_state(world_root: &Path) -> Result<LocalStatePaths> {
    let paths = local_state_paths(world_root);
    paths
        .reset_runtime_state()
        .with_context(|| format!("reset local runtime state under {}", paths.root().display()))?;
    Ok(paths)
}

pub fn local_kernel_config(world_root: &Path) -> Result<KernelConfig> {
    let cache_dir = local_state_paths(world_root).wasmtime_cache_dir();
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("create cache dir {}", cache_dir.display()))?;
    Ok(KernelConfig {
        module_cache_dir: Some(cache_dir),
        eager_module_load: true,
        secret_resolver: None,
        allow_placeholder_secrets: false,
        cell_cache_size: aos_kernel::world::DEFAULT_CELL_CACHE_SIZE,
        universe_id: uuid::Uuid::nil(),
    })
}

pub fn build_runtime_workflow_harness_from_authored_paths(
    workflow: impl Into<String>,
    air_dir: &Path,
    workflow_dir: Option<&Path>,
    import_roots: &[PathBuf],
    scratch_root: &Path,
    force_build: bool,
    build_profile: WorkflowBuildProfile,
    effect_mode: EffectMode,
) -> Result<WorkflowHarness<MemStore>> {
    build_runtime_workflow_harness_from_authored_paths_with_config(
        workflow,
        air_dir,
        workflow_dir,
        import_roots,
        scratch_root,
        force_build,
        build_profile,
        effect_mode,
        WorldConfig::default(),
        EffectAdapterConfig::default(),
        KernelConfig::default(),
    )
}

fn secret_resolver_from_authored_manifest(
    loaded: &LoadedManifest,
    sync_root: Option<&Path>,
    secret_map: Option<&Path>,
    explicit_bindings: Option<HashMap<String, Vec<u8>>>,
) -> Result<Option<SharedSecretResolver>> {
    let required_bindings = loaded
        .secrets
        .iter()
        .map(|secret| secret.binding_id.clone())
        .collect::<BTreeSet<_>>();
    secret_resolver_for_bindings(required_bindings, sync_root, secret_map, explicit_bindings)
}

fn secret_resolver_for_bindings(
    required_bindings: BTreeSet<String>,
    sync_root: Option<&Path>,
    secret_map: Option<&Path>,
    explicit_bindings: Option<HashMap<String, Vec<u8>>>,
) -> Result<Option<SharedSecretResolver>> {
    let mut values = explicit_bindings.unwrap_or_default();

    if let Some(sync_root) = sync_root {
        let missing = required_bindings
            .iter()
            .filter(|binding| !values.contains_key(binding.as_str()))
            .cloned()
            .collect::<BTreeSet<_>>();
        if !missing.is_empty() {
            values.extend(load_available_secret_value_map(
                sync_root, secret_map, &missing,
            )?);
        }
    }

    if required_bindings.is_empty() {
        if values.is_empty() {
            return Ok(None);
        }
        return Ok(Some(Arc::new(MapSecretResolver::new(values))));
    }

    Ok(Some(Arc::new(MapSecretResolver::new(values))))
}

pub fn build_runtime_workflow_harness_from_authored_paths_with_config(
    workflow: impl Into<String>,
    air_dir: &Path,
    workflow_dir: Option<&Path>,
    import_roots: &[PathBuf],
    scratch_root: &Path,
    force_build: bool,
    build_profile: WorkflowBuildProfile,
    effect_mode: EffectMode,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: KernelConfig,
) -> Result<WorkflowHarness<MemStore>> {
    build_runtime_workflow_harness_from_authored_paths_with_secret_config(
        workflow,
        air_dir,
        workflow_dir,
        import_roots,
        scratch_root,
        force_build,
        build_profile,
        effect_mode,
        world_config,
        adapter_config,
        kernel_config,
        None,
        None,
        None,
    )
}

pub fn build_runtime_workflow_harness_from_authored_paths_with_secret_config(
    workflow: impl Into<String>,
    air_dir: &Path,
    workflow_dir: Option<&Path>,
    import_roots: &[PathBuf],
    scratch_root: &Path,
    force_build: bool,
    build_profile: WorkflowBuildProfile,
    effect_mode: EffectMode,
    mut world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    mut kernel_config: KernelConfig,
    sync_root: Option<&Path>,
    secret_map: Option<&Path>,
    explicit_secret_bindings: Option<HashMap<String, Vec<u8>>>,
) -> Result<WorkflowHarness<MemStore>> {
    let (store, loaded) = build_loaded_manifest_from_authored_paths(
        air_dir,
        workflow_dir,
        import_roots,
        scratch_root,
        force_build,
        build_profile,
    )?;
    let has_secret_config = sync_root.is_some()
        || explicit_secret_bindings
            .as_ref()
            .is_some_and(|values| !values.is_empty());
    if kernel_config.secret_resolver.is_some() && has_secret_config {
        anyhow::bail!(
            "kernel_config.secret_resolver cannot be combined with workflow secret config"
        );
    }
    if kernel_config.secret_resolver.is_none() {
        kernel_config.secret_resolver = secret_resolver_from_authored_manifest(
            &loaded,
            sync_root,
            secret_map,
            explicit_secret_bindings,
        )?;
    }
    if world_config.module_cache_dir.is_none() {
        world_config.module_cache_dir = WorldConfig::from_env().module_cache_dir;
    }
    Ok(HarnessBuilder::ephemeral(Arc::new(store), loaded)
        .world_config(world_config)
        .adapter_config(adapter_config)
        .kernel_config(kernel_config)
        .effect_mode(effect_mode)
        .build_workflow(workflow.into())?)
}

pub fn build_runtime_workflow_harness_from_workflow_dir(
    workflow: impl Into<String>,
    workflow_dir: &Path,
    air_dir: Option<&Path>,
    import_roots: &[PathBuf],
    scratch_root: &Path,
    force_build: bool,
    build_profile: WorkflowBuildProfile,
    effect_mode: EffectMode,
) -> Result<WorkflowHarness<MemStore>> {
    let air_dir = air_dir
        .map(Path::to_path_buf)
        .or_else(|| workflow_dir.parent().map(|parent| parent.join("air")))
        .ok_or_else(|| anyhow!("workflow_dir has no parent; pass air_dir explicitly"))?;
    build_runtime_workflow_harness_from_authored_paths(
        workflow,
        &air_dir,
        Some(workflow_dir),
        import_roots,
        scratch_root,
        force_build,
        build_profile,
        effect_mode,
    )
}

pub fn build_world_harness_from_bundle(
    store: Arc<FsCas>,
    bundle: WorldBundle,
    state_paths: Option<&LocalStatePaths>,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: KernelConfig,
    effect_mode: EffectMode,
) -> Result<EmbeddedWorldHarness> {
    build_embedded_world_harness_from_bundle_with_world_id(
        store,
        bundle,
        state_paths,
        WorldId::from(Uuid::new_v4()),
        world_config,
        adapter_config,
        kernel_config,
        effect_mode,
    )
}

fn build_embedded_world_harness_from_bundle_with_world_id(
    store: Arc<FsCas>,
    bundle: WorldBundle,
    state_paths: Option<&LocalStatePaths>,
    world_id: WorldId,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: KernelConfig,
    effect_mode: EffectMode,
) -> Result<EmbeddedWorldHarness> {
    let imported = import_genesis(store.as_ref(), &bundle).context("store world bundle")?;
    let paths =
        state_paths.ok_or_else(|| anyhow!("embedded world harness requires local state paths"))?;
    EmbeddedWorldHarness::bootstrap(
        paths.clone(),
        world_id,
        imported.manifest_hash,
        world_config,
        adapter_config,
        kernel_config,
        effect_mode,
    )
    .map_err(|err| anyhow!(err.to_string()))
}

pub fn bootstrap_seeded_local_world_harness(
    world_root: &Path,
    reset: bool,
    force_build: bool,
    sync_secrets: bool,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    mut kernel_config: KernelConfig,
    effect_mode: EffectMode,
) -> Result<SeededLocalHarness> {
    let world_id = WorldId::from(Uuid::new_v4());
    if reset {
        reset_local_runtime_state(world_root)?;
    }
    let (store, bundle, warnings) = build_bundle_from_local_world(world_root, force_build)
        .with_context(|| format!("build bundle from {}", world_root.display()))?;
    if sync_secrets && kernel_config.secret_resolver.is_none() {
        let required_bindings = bundle
            .secrets
            .iter()
            .map(|secret| secret.binding_id.clone())
            .collect::<BTreeSet<_>>();
        kernel_config.secret_resolver =
            secret_resolver_for_bindings(required_bindings, Some(world_root), None, None)?;
    }
    let store = Arc::new(store);
    let paths = local_state_paths(world_root);
    let harness = build_embedded_world_harness_from_bundle_with_world_id(
        Arc::clone(&store),
        bundle,
        Some(&paths),
        world_id,
        world_config,
        adapter_config,
        kernel_config,
        effect_mode,
    )?;
    Ok(SeededLocalHarness {
        world_id,
        harness,
        store,
        warnings,
    })
}

pub fn bootstrap_seeded_persisted_world_harness(
    world_root: &Path,
    reset: bool,
    force_build: bool,
    sync_secrets: bool,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: KernelConfig,
    effect_mode: EffectMode,
) -> Result<SeededLocalHarness> {
    bootstrap_seeded_local_world_harness(
        world_root,
        reset,
        force_build,
        sync_secrets,
        world_config,
        adapter_config,
        kernel_config,
        effect_mode,
    )
}
