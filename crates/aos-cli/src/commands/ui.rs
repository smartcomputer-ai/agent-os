//! `aos ui` commands.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};
use clap::{Args, Subcommand};
use serde_json::Value as JsonValue;
use walkdir::WalkDir;

use aos_air_types::{AirNode, DefModule, HashRef, Manifest, RoutingEvent, SchemaRef, builtins};
use aos_cbor::Hash;
use aos_kernel::governance::ManifestPatch;
use aos_store::{FsStore, Store};
use aos_sys::{HttpPublishRegistry, HttpPublishRule, HttpPublishSet, WorkspaceRef};

use crate::opts::{WorldOpts, resolve_dirs};
use crate::output::print_success;
use crate::util::{load_world_env, resolve_sys_module_wasm_hash};

use super::workspace_sync::{SyncPushOptions, sync_workspace_push};
use super::{create_host, prepare_world};

const PUBLISH_EVENT: &str = "sys/HttpPublishSet@1";
const PUBLISH_REDUCER: &str = "sys/HttpPublish@1";
const WORKSPACE_EVENT: &str = "sys/WorkspaceCommit@1";
const WORKSPACE_REDUCER: &str = "sys/Workspace@1";
const DEFAULT_APP_DIR: &str = "apps/shell";
const DEFAULT_WORKSPACE: &str = "shell";
const DEFAULT_ROUTE: &str = "/";
const DEFAULT_DOC: &str = "index.html";
const CACHE_ETAG: &str = "etag";
const CACHE_HTML: &str = "no-cache";
const CACHE_ASSET: &str = "public, max-age=31536000, immutable";

#[derive(Args, Debug)]
pub struct UiArgs {
    #[command(subcommand)]
    pub cmd: UiCommand,
}

#[derive(Subcommand, Debug)]
pub enum UiCommand {
    /// Build and install the shell UI into a workspace
    Install(UiInstallArgs),
}

#[derive(Args, Debug)]
pub struct UiInstallArgs {
    /// Shell app directory
    #[arg(long, default_value = DEFAULT_APP_DIR)]
    pub app_dir: PathBuf,

    /// Shell build output directory (defaults to <app-dir>/dist)
    #[arg(long)]
    pub dist_dir: Option<PathBuf>,

    /// Target workspace name
    #[arg(long, default_value = DEFAULT_WORKSPACE)]
    pub workspace: String,

    /// Route prefix to publish the shell from
    #[arg(long, default_value = DEFAULT_ROUTE)]
    pub route: String,

    /// Overwrite existing publish rules for the route
    #[arg(long)]
    pub force: bool,
}

pub async fn cmd_ui(opts: &WorldOpts, args: &UiArgs) -> Result<()> {
    match &args.cmd {
        UiCommand::Install(inner) => ui_install(opts, inner).await,
    }
}

async fn ui_install(opts: &WorldOpts, args: &UiInstallArgs) -> Result<()> {
    let app_dir = resolve_path(&args.app_dir)?;
    if !app_dir.exists() {
        anyhow::bail!("app dir not found: {}", app_dir.display());
    }
    if !app_dir.is_dir() {
        anyhow::bail!("app path is not a directory: {}", app_dir.display());
    }
    let dist_dir = resolve_path(args.dist_dir.as_ref().unwrap_or(&app_dir.join("dist")))?;
    run_shell_build(&app_dir)?;
    ensure_dir_exists(&dist_dir)?;

    let workspace = args.workspace.trim();
    if workspace.is_empty() {
        anyhow::bail!("workspace name is required");
    }
    let route = normalize_route(&args.route)?;

    let dirs = resolve_dirs(opts)?;
    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let mut host = create_host(store.clone(), loaded, &dirs, opts)?;
    ensure_sys_support(&mut host, store.as_ref(), &dirs)?;

    let annotations = build_shell_annotations(&dist_dir)?;
    let ignore = vec!["!**".to_string()];
    let stats = sync_workspace_push(
        &mut host,
        store.as_ref(),
        workspace,
        &dist_dir,
        &ignore,
        &annotations,
        &SyncPushOptions {
            prune: true,
            message: None,
        },
    )?;

    apply_publish_rule(&mut host, workspace, &route, args.force)?;
    host.kernel_mut()
        .create_snapshot()
        .context("create snapshot")?;

    print_success(
        opts,
        serde_json::json!({
            "workspace": workspace,
            "route": route,
            "dist": dist_dir.display().to_string(),
            "writes": stats.writes,
            "removes": stats.removes,
            "annotations": stats.annotations,
            "published": true
        }),
        None,
        vec![],
    )
}

fn run_shell_build(app_dir: &Path) -> Result<()> {
    let status = Command::new("npm")
        .arg("run")
        .arg("build")
        .current_dir(app_dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("run npm build in {}", app_dir.display()))?;
    if !status.success() {
        anyhow::bail!("shell build failed");
    }
    Ok(())
}

fn ensure_dir_exists(dir: &Path) -> Result<()> {
    if !dir.exists() {
        anyhow::bail!("dist dir not found: {}", dir.display());
    }
    if !dir.is_dir() {
        anyhow::bail!("dist path is not a directory: {}", dir.display());
    }
    Ok(())
}

fn resolve_path(path: &Path) -> Result<PathBuf> {
    if path.is_relative() {
        Ok(std::env::current_dir()
            .context("get current directory")?
            .join(path))
    } else {
        Ok(path.to_path_buf())
    }
}

fn normalize_route(route: &str) -> Result<String> {
    let trimmed = route.trim();
    if trimmed.is_empty() {
        anyhow::bail!("route is required");
    }
    if trimmed.starts_with('/') {
        Ok(trimmed.to_string())
    } else {
        Ok(format!("/{trimmed}"))
    }
}

fn build_shell_annotations(
    dist_dir: &Path,
) -> Result<BTreeMap<String, BTreeMap<String, JsonValue>>> {
    let mut out: BTreeMap<String, BTreeMap<String, JsonValue>> = BTreeMap::new();
    for entry in WalkDir::new(dist_dir).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(dist_dir)
            .with_context(|| format!("strip prefix {}", dist_dir.display()))?;
        let rel_str = encode_workspace_path(rel)?;
        let content_type = content_type_for_path(entry.path());
        let cache_control = cache_control_for_path(entry.path());
        let values = out.entry(rel_str).or_default();
        values.insert(
            "http.content-type".to_string(),
            JsonValue::String(content_type.to_string()),
        );
        values.insert(
            "http.cache-control".to_string(),
            JsonValue::String(cache_control.to_string()),
        );
    }
    Ok(out)
}

fn content_type_for_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn cache_control_for_path(path: &Path) -> &'static str {
    let is_html = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("html"))
        .unwrap_or(false);
    if is_html { CACHE_HTML } else { CACHE_ASSET }
}

fn encode_workspace_path(path: &Path) -> Result<String> {
    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(name) => {
                let raw = name
                    .to_str()
                    .ok_or_else(|| anyhow!("non-UTF-8 path segment"))?;
                segments.push(encode_segment(raw));
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                anyhow::bail!("parent path components are not allowed");
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                anyhow::bail!("absolute paths are not allowed");
            }
        }
    }
    if segments.is_empty() {
        anyhow::bail!("path is empty");
    }
    Ok(segments.join("/"))
}

fn encode_segment(raw: &str) -> String {
    if !raw.is_empty()
        && !raw.starts_with('~')
        && raw.chars().all(|c| {
            matches!(c, 'a'..='z'
            | 'A'..='Z'
            | '0'..='9'
            | '.'
            | '_'
            | '-'
            | '~')
        })
    {
        return raw.to_string();
    }
    let mut out = String::from("~");
    for byte in raw.as_bytes() {
        out.push_str(&format!("{:02X}", byte));
    }
    out
}

fn apply_publish_rule(
    host: &mut aos_host::host::WorldHost<FsStore>,
    workspace: &str,
    route: &str,
    force: bool,
) -> Result<()> {
    let registry = load_publish_registry(host)?;
    let mut existing: Vec<String> = registry
        .rules
        .iter()
        .filter(|(_, rule)| rule.route_prefix == route)
        .map(|(id, _)| id.clone())
        .collect();

    if !force && !existing.is_empty() {
        existing.sort();
        anyhow::bail!(
            "publish route '{}' already exists (rule ids: {}); use --force to overwrite",
            route,
            existing.join(", ")
        );
    }

    for id in existing.iter().filter(|id| id.as_str() != workspace) {
        submit_publish_event(
            host,
            HttpPublishSet {
                id: id.to_string(),
                rule: None,
            },
        )?;
    }

    let rule = HttpPublishRule {
        route_prefix: route.to_string(),
        workspace: WorkspaceRef {
            workspace: workspace.to_string(),
            version: None,
            path: None,
        },
        default_doc: Some(DEFAULT_DOC.to_string()),
        allow_dir_listing: false,
        cache: CACHE_ETAG.to_string(),
    };
    submit_publish_event(
        host,
        HttpPublishSet {
            id: workspace.to_string(),
            rule: Some(rule),
        },
    )
}

fn load_publish_registry(host: &aos_host::host::WorldHost<FsStore>) -> Result<HttpPublishRegistry> {
    let Some(bytes) = host.state(PUBLISH_REDUCER, None) else {
        return Ok(HttpPublishRegistry::default());
    };
    let registry: HttpPublishRegistry =
        serde_cbor::from_slice(&bytes).context("decode publish registry")?;
    Ok(registry)
}

fn submit_publish_event(
    host: &mut aos_host::host::WorldHost<FsStore>,
    event: HttpPublishSet,
) -> Result<()> {
    let payload = serde_cbor::to_vec(&event).context("encode publish event")?;
    host.kernel_mut()
        .submit_domain_event_result(PUBLISH_EVENT, payload)
        .map_err(|e| anyhow!("publish update failed: {e}"))
}

fn ensure_sys_support(
    host: &mut aos_host::host::WorldHost<FsStore>,
    store: &FsStore,
    dirs: &crate::opts::ResolvedDirs,
) -> Result<()> {
    let base_hash = host.kernel().manifest_hash();
    let mut manifest: Manifest = store
        .get_node(base_hash)
        .context("load manifest from store")?;
    let mut changed = false;
    let mut nodes: Vec<AirNode> = Vec::new();

    for name in required_schema_refs() {
        if manifest.schemas.iter().any(|entry| entry.name == name) {
            continue;
        }
        let builtin = builtins::find_builtin_schema(&name)
            .ok_or_else(|| anyhow!("missing builtin schema '{name}'"))?;
        manifest.schemas.push(aos_air_types::NamedRef {
            name,
            hash: builtin.hash_ref.clone(),
        });
        changed = true;
    }

    for name in required_module_refs() {
        let (hash_ref, node) = build_sys_module_node(store, dirs, &name)?;
        match manifest.modules.iter_mut().find(|entry| entry.name == name) {
            Some(entry) => {
                if entry.hash != hash_ref {
                    entry.hash = hash_ref;
                    changed = true;
                    nodes.push(node);
                }
            }
            None => {
                manifest.modules.push(aos_air_types::NamedRef {
                    name,
                    hash: hash_ref,
                });
                changed = true;
                nodes.push(node);
            }
        }
    }

    let routing = manifest
        .routing
        .get_or_insert_with(|| aos_air_types::Routing {
            subscriptions: Vec::new(),
            inboxes: Vec::new(),
        });

    let mut next_events = Vec::new();
    let mut saw_publish = false;
    let mut saw_workspace = false;
    for route in &routing.subscriptions {
        if route.event.as_str() == PUBLISH_EVENT && route.module == PUBLISH_REDUCER {
            saw_publish = true;
            next_events.push(route.clone());
            continue;
        }
        if route.event.as_str() == WORKSPACE_EVENT && route.module == WORKSPACE_REDUCER {
            saw_workspace = true;
            if route.key_field.as_deref() != Some("workspace") {
                let mut fixed = route.clone();
                fixed.key_field = Some("workspace".to_string());
                next_events.push(fixed);
                changed = true;
            } else {
                next_events.push(route.clone());
            }
            continue;
        }
        next_events.push(route.clone());
    }
    if !saw_publish {
        next_events.push(RoutingEvent {
            event: SchemaRef::new(PUBLISH_EVENT)?,
            module: PUBLISH_REDUCER.to_string(),
            key_field: None,
        });
        changed = true;
    }
    if !saw_workspace {
        next_events.push(RoutingEvent {
            event: SchemaRef::new(WORKSPACE_EVENT)?,
            module: WORKSPACE_REDUCER.to_string(),
            key_field: Some("workspace".to_string()),
        });
        changed = true;
    }
    if changed {
        routing.subscriptions = next_events;
    }

    if !changed {
        return Ok(());
    }
    let patch = ManifestPatch { manifest, nodes };
    host.kernel_mut()
        .apply_patch_direct(patch)
        .context("apply sys manifest patch")?;
    Ok(())
}

fn build_sys_module_node(
    store: &FsStore,
    dirs: &crate::opts::ResolvedDirs,
    name: &str,
) -> Result<(HashRef, AirNode)> {
    let builtin = builtins::find_builtin_module(name)
        .ok_or_else(|| anyhow!("missing builtin module '{name}'"))?;
    let wasm_hash = resolve_sys_module_wasm_hash(store, &dirs.store_root, &dirs.world, name)?;
    let mut module: DefModule = builtin.module.clone();
    module.wasm_hash = wasm_hash;
    let node = AirNode::Defmodule(module);
    let hash = Hash::of_cbor(&node).context("hash system module")?;
    let hash_ref = HashRef::new(hash.to_hex()).context("create module hash ref")?;
    Ok((hash_ref, node))
}

fn required_schema_refs() -> Vec<String> {
    vec![
        "sys/ReducerContext@1".to_string(),
        "sys/WorkspaceName@1".to_string(),
        "sys/WorkspaceCommitMeta@1".to_string(),
        "sys/WorkspaceHistory@1".to_string(),
        "sys/WorkspaceCommit@1".to_string(),
        "sys/WorkspaceRef@1".to_string(),
        "sys/HttpPublishRule@1".to_string(),
        "sys/HttpPublishRegistry@1".to_string(),
        "sys/HttpPublishSet@1".to_string(),
    ]
}

fn required_module_refs() -> Vec<String> {
    vec![WORKSPACE_REDUCER.to_string(), PUBLISH_REDUCER.to_string()]
}
