use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_air_types::HashRef;
use aos_host::config::HostConfig;
use aos_host::host::{ExternalEvent, WorldHost};
use aos_host::manifest_loader;
use aos_host::modes::batch::BatchRunner;
use aos_host::util::{has_placeholder_modules, is_placeholder_hash, patch_modules, reset_journal};
use aos_kernel::KernelConfig;
use aos_store::{FsStore, Store};
use aos_wasm_build::{BuildRequest, Builder};
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use serde_json::Value as JsonValue;

#[derive(Parser, Debug)]
#[command(name = "aos", version, about = "AgentOS CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// World management commands
    World {
        #[command(subcommand)]
        cmd: WorldCommand,
    },
}

#[derive(Subcommand, Debug)]
enum WorldCommand {
    /// Initialize a world directory
    Init {
        /// Path to world directory
        path: PathBuf,
    },
    /// Run a single batch step
    Step {
        /// Path to world directory
        path: PathBuf,

        /// AIR assets directory (default: <path>/air)
        #[arg(long)]
        air: Option<PathBuf>,

        /// Reducer crate directory (default: <path>/reducer)
        #[arg(long)]
        reducer: Option<PathBuf>,

        /// Store/journal directory (default: <path>/.aos)
        #[arg(long)]
        store: Option<PathBuf>,

        /// Module name to patch with compiled WASM (default: all placeholders)
        #[arg(long)]
        module: Option<String>,

        /// Event schema to inject
        #[arg(long)]
        event: Option<String>,

        /// Event value as JSON
        #[arg(long)]
        value: Option<String>,

        /// Force reducer recompilation
        #[arg(long)]
        force_build: bool,

        /// Clear journal before step
        #[arg(long = "reset-journal")]
        do_reset_journal: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::World { cmd } => match cmd {
            WorldCommand::Init { path } => cmd_world_init(path),
            WorldCommand::Step {
                path,
                air,
                reducer,
                store,
                module,
                event,
                value,
                force_build,
                do_reset_journal,
            } => {
                cmd_world_step(
                    path,
                    air,
                    reducer,
                    store,
                    module,
                    event,
                    value,
                    force_build,
                    do_reset_journal,
                )
                .await
            }
        },
    }
}

fn cmd_world_init(path: PathBuf) -> Result<()> {
    fs::create_dir_all(&path)?;
    fs::create_dir_all(path.join(".aos"))?;
    fs::create_dir_all(path.join("air"))?;
    fs::create_dir_all(path.join("reducer/src"))?;

    // Write minimal manifest
    let manifest = r#"{
  "$kind": "manifest",
  "air_version": "1",
  "schemas": [],
  "modules": [],
  "plans": [],
  "caps": [],
  "policies": [],
  "effects": [],
  "triggers": []
}"#;
    fs::write(path.join("air/manifest.air.json"), manifest)?;

    println!("World initialized at {}", path.display());
    println!("  AIR assets: {}", path.join("air").display());
    println!("  Reducer:    {}", path.join("reducer").display());
    println!("  Store:      {}", path.join(".aos").display());
    Ok(())
}

async fn cmd_world_step(
    path: PathBuf,
    air: Option<PathBuf>,
    reducer: Option<PathBuf>,
    store_path: Option<PathBuf>,
    module: Option<String>,
    event: Option<String>,
    value: Option<String>,
    force_build: bool,
    do_reset_journal: bool,
) -> Result<()> {
    // Validate world directory
    if !path.exists() {
        anyhow::bail!("world directory '{}' not found", path.display());
    }
    if !path.is_dir() {
        anyhow::bail!("'{}' is not a directory", path.display());
    }

    // Resolve directories with defaults
    // If paths are relative, make them relative to the world directory
    let air_dir = match air {
        Some(p) if p.is_relative() => path.join(p),
        Some(p) => p,
        None => path.join("air"),
    };
    let reducer_dir = match reducer {
        Some(p) if p.is_relative() => path.join(p),
        Some(p) => p,
        None => path.join("reducer"),
    };
    // store_root is where .aos/ will be created (defaults to world directory)
    let store_root = match store_path {
        Some(p) if p.is_relative() => path.join(p),
        Some(p) => p,
        None => path.clone(),
    };

    // Optionally reset journal (journal is at <store_root>/.aos/journal/)
    if do_reset_journal {
        reset_journal(&store_root)?;
        println!("Journal cleared");
    }

    // Open store (creates .aos/store/ inside store_root)
    let store = Arc::new(FsStore::open(&store_root).context("open store")?);

    // Compile reducer if present
    let wasm_hash = if reducer_dir.exists() {
        println!("Compiling reducer from {}...", reducer_dir.display());
        let hash = compile_reducer(&reducer_dir, &store_root, &store, force_build)?;
        println!("Reducer compiled: {}", hash.as_str());
        Some(hash)
    } else {
        None
    };

    // Load manifest from AIR assets
    let mut loaded = manifest_loader::load_from_assets(store.clone(), &air_dir)
        .context("load manifest from assets")?
        .ok_or_else(|| anyhow!("no manifest found in {}", air_dir.display()))?;

    // Patch module hashes
    if let Some(hash) = &wasm_hash {
        let patched = patch_module_hashes(&mut loaded, hash, module.as_deref())?;
        if patched > 0 {
            println!("Patched {} module(s) with WASM hash", patched);
        }
    } else if has_placeholder_modules(&loaded) {
        anyhow::bail!(
            "manifest has modules with placeholder hashes but no reducer/ found; \
             use --reducer to specify reducer crate"
        );
    }

    // Create host and run
    let host_config = HostConfig::default();
    let kernel_config = make_kernel_config(&store_root)?;
    let host =
        WorldHost::from_loaded_manifest(store, loaded, &store_root, host_config, kernel_config)?;
    let mut runner = BatchRunner::new(host);

    // Build events
    let mut events = Vec::new();
    if let Some(schema) = event {
        let json = value.unwrap_or_else(|| "{}".to_string());
        let parsed: JsonValue = serde_json::from_str(&json).context("parse event value as JSON")?;
        let cbor = serde_cbor::to_vec(&parsed).context("encode event value as CBOR")?;
        events.push(ExternalEvent::DomainEvent {
            schema,
            value: cbor,
        });
    }

    // Run step
    let res = runner.step(events).await?;
    println!(
        "Step complete: events={} effects={} receipts={}",
        res.events_injected, res.cycle.effects_dispatched, res.cycle.receipts_applied
    );
    Ok(())
}

fn compile_reducer(
    reducer_dir: &Path,
    store_root: &Path,
    store: &FsStore,
    force_build: bool,
) -> Result<HashRef> {
    let cache_dir = store_root.join(".aos/cache/modules");
    fs::create_dir_all(&cache_dir).context("create module cache directory")?;

    let utf_path = Utf8PathBuf::from_path_buf(reducer_dir.to_path_buf())
        .map_err(|p| anyhow!("reducer path is not UTF-8: {}", p.display()))?;

    let mut request = BuildRequest::new(utf_path);
    request.cache_dir = Some(cache_dir);
    request.use_cache = !force_build;
    request.config.release = false;

    let artifact = Builder::compile(request).context("compile reducer")?;
    let hash = store
        .put_blob(&artifact.wasm_bytes)
        .context("store wasm blob")?;
    HashRef::new(hash.to_hex()).context("create hash ref")
}

fn patch_module_hashes(
    loaded: &mut aos_kernel::LoadedManifest,
    wasm_hash: &HashRef,
    specific_module: Option<&str>,
) -> Result<usize> {
    let patched = match specific_module {
        Some(target) => patch_modules(loaded, wasm_hash, |name, _| name == target),
        None => patch_modules(loaded, wasm_hash, |_, m| is_placeholder_hash(m)),
    };

    if let Some(target) = specific_module {
        if patched == 0 {
            anyhow::bail!("module '{}' not found in manifest", target);
        }
    }

    Ok(patched)
}

fn make_kernel_config(store_root: &Path) -> Result<KernelConfig> {
    let cache_dir = store_root.join(".aos/cache/wasmtime");
    fs::create_dir_all(&cache_dir).context("create wasmtime cache directory")?;
    Ok(KernelConfig {
        module_cache_dir: Some(cache_dir),
        eager_module_load: true,
        secret_resolver: None,
        allow_placeholder_secrets: true,
    })
}
