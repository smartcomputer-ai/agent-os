//! `aos ws` commands.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use aos_effects::{EffectKind, IntentBuilder, ReceiptStatus};
use aos_host::control::{ControlClient, RequestEnvelope};
use aos_host::host::WorldHost;
use aos_store::FsStore;
use aos_sys::{WorkspaceCommit, WorkspaceCommitMeta, WorkspaceHistory};
use base64::Engine;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::input::parse_input_value;
use crate::opts::{Mode, WorldOpts, resolve_dirs};
use crate::output::print_success;
use crate::util::load_world_env;

use super::{create_host, prepare_world, should_use_control, try_control_client};

const WORKSPACE_CAP: &str = "sys/workspace@1";
const WORKSPACE_EVENT: &str = "sys/WorkspaceCommit@1";
const WORKSPACE_REDUCER: &str = "sys/Workspace@1";

#[derive(Args, Debug)]
pub struct WorkspaceArgs {
    #[command(subcommand)]
    pub cmd: WorkspaceCommand,
}

#[derive(Subcommand, Debug)]
pub enum WorkspaceCommand {
    /// Resolve a workspace ref
    Resolve(WorkspaceResolveArgs),
    /// List workspaces or paths
    Ls(WorkspaceLsArgs),
    /// Read file contents
    Cat(WorkspaceCatArgs),
    /// Stat a path
    Stat(WorkspaceStatArgs),
    /// Write bytes to a path
    Write(WorkspaceWriteArgs),
    /// Remove a path
    Rm(WorkspaceRmArgs),
    /// Diff two workspace roots
    Diff(WorkspaceDiffArgs),
    /// Show workspace commit history
    Log(WorkspaceLogArgs),
    /// Workspace annotations
    Ann(WorkspaceAnnArgs),
}

#[derive(Args, Debug)]
pub struct WorkspaceResolveArgs {
    /// Workspace ref: <workspace>[@<version>][/path]
    pub reference: String,
}

#[derive(Args, Debug)]
pub struct WorkspaceLsArgs {
    /// Workspace ref: <workspace>[@<version>][/path]
    pub reference: Option<String>,
    /// Listing scope: dir or subtree
    #[arg(long)]
    pub scope: Option<String>,
    /// Max number of entries to return (0 = no limit)
    #[arg(long)]
    pub limit: Option<u64>,
    /// Cursor to continue listing
    #[arg(long)]
    pub cursor: Option<String>,
}

#[derive(Args, Debug)]
pub struct WorkspaceCatArgs {
    /// Workspace ref: <workspace>[@<version>]/path
    pub reference: String,
    /// Read byte range START:END
    #[arg(long)]
    pub range: Option<String>,
    /// Emit raw bytes to stdout
    #[arg(long, conflicts_with = "out")]
    pub raw: bool,
    /// Write bytes to file
    #[arg(long, conflicts_with = "raw")]
    pub out: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct WorkspaceStatArgs {
    /// Workspace ref: <workspace>[@<version>]/path
    pub reference: String,
}

#[derive(Args, Debug)]
#[command(group(
    clap::ArgGroup::new("ws_input")
        .required(true)
        .args(&["input", "text_in", "json_in"])
))]
pub struct WorkspaceWriteArgs {
    /// Workspace ref: <workspace>[@<version>]/path
    pub reference: String,
    /// Input file path or @- for stdin
    #[arg(long = "in")]
    pub input: Option<String>,
    /// Text input (UTF-8)
    #[arg(long = "text-in")]
    pub text_in: Option<String>,
    /// JSON input (literal, @file, or @-)
    #[arg(long = "json-in")]
    pub json_in: Option<String>,
    /// File mode (644 or 755)
    #[arg(long = "file-mode")]
    pub file_mode: Option<String>,
    /// Commit owner
    #[arg(long)]
    pub owner: Option<String>,
}

#[derive(Args, Debug)]
pub struct WorkspaceRmArgs {
    /// Workspace ref: <workspace>[@<version>]/path
    pub reference: String,
    /// Commit owner
    #[arg(long)]
    pub owner: Option<String>,
}

#[derive(Args, Debug)]
pub struct WorkspaceDiffArgs {
    /// Workspace ref A: <workspace>[@<version>][/path]
    pub ref_a: String,
    /// Workspace ref B: <workspace>[@<version>][/path]
    pub ref_b: String,
    /// Limit diff to a path prefix
    #[arg(long)]
    pub prefix: Option<String>,
}

#[derive(Args, Debug)]
pub struct WorkspaceLogArgs {
    /// Workspace name
    pub workspace: String,
}

#[derive(Args, Debug)]
pub struct WorkspaceAnnArgs {
    #[command(subcommand)]
    pub cmd: WorkspaceAnnCommand,
}

#[derive(Subcommand, Debug)]
pub enum WorkspaceAnnCommand {
    /// Get annotations
    Get(WorkspaceAnnGetArgs),
    /// Set annotations
    Set(WorkspaceAnnSetArgs),
    /// Delete annotations
    Del(WorkspaceAnnDelArgs),
}

#[derive(Args, Debug)]
pub struct WorkspaceAnnGetArgs {
    /// Workspace ref: <workspace>[@<version>][/path]
    pub reference: String,
}

#[derive(Args, Debug)]
pub struct WorkspaceAnnSetArgs {
    /// Workspace ref: <workspace>[@<version>][/path]
    pub reference: String,
    /// Annotation entries: <key>=<hash>
    pub entries: Vec<String>,
    /// Commit owner
    #[arg(long)]
    pub owner: Option<String>,
}

#[derive(Args, Debug)]
pub struct WorkspaceAnnDelArgs {
    /// Workspace ref: <workspace>[@<version>][/path]
    pub reference: String,
    /// Annotation keys to delete
    pub keys: Vec<String>,
    /// Commit owner
    #[arg(long)]
    pub owner: Option<String>,
}

#[derive(Debug, Clone)]
struct WorkspaceRef {
    workspace: String,
    version: Option<u64>,
    path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceResolveParams {
    workspace: String,
    version: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceResolveReceipt {
    exists: bool,
    resolved_version: Option<u64>,
    head: Option<u64>,
    root_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceEmptyRootParams {
    workspace: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceEmptyRootReceipt {
    root_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceListParams {
    root_hash: String,
    path: Option<String>,
    scope: Option<String>,
    cursor: Option<String>,
    limit: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceListEntry {
    path: String,
    kind: String,
    hash: Option<String>,
    size: Option<u64>,
    mode: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceListReceipt {
    entries: Vec<WorkspaceListEntry>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceReadRefParams {
    root_hash: String,
    path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceRefEntry {
    kind: String,
    hash: String,
    size: u64,
    mode: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceReadBytesParams {
    root_hash: String,
    path: String,
    range: Option<WorkspaceReadBytesRange>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceReadBytesRange {
    start: u64,
    end: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceWriteBytesParams {
    root_hash: String,
    path: String,
    bytes: Vec<u8>,
    mode: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceWriteBytesReceipt {
    new_root_hash: String,
    blob_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceRemoveParams {
    root_hash: String,
    path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceRemoveReceipt {
    new_root_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceDiffParams {
    root_a: String,
    root_b: String,
    prefix: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceDiffReceipt {
    changes: Vec<WorkspaceDiffChange>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceDiffChange {
    path: String,
    kind: String,
    old_hash: Option<String>,
    new_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(transparent)]
struct WorkspaceAnnotations(BTreeMap<String, String>);

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(transparent)]
struct WorkspaceAnnotationsPatch(BTreeMap<String, Option<String>>);

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsGetParams {
    root_hash: String,
    path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsGetReceipt {
    annotations: Option<WorkspaceAnnotations>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsSetParams {
    root_hash: String,
    path: Option<String>,
    annotations_patch: WorkspaceAnnotationsPatch,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsSetReceipt {
    new_root_hash: String,
    annotations_hash: String,
}


pub async fn cmd_ws(opts: &WorldOpts, args: &WorkspaceArgs) -> Result<()> {
    match &args.cmd {
        WorkspaceCommand::Resolve(a) => ws_resolve(opts, a).await,
        WorkspaceCommand::Ls(a) => ws_ls(opts, a).await,
        WorkspaceCommand::Cat(a) => ws_cat(opts, a).await,
        WorkspaceCommand::Stat(a) => ws_stat(opts, a).await,
        WorkspaceCommand::Write(a) => ws_write(opts, a).await,
        WorkspaceCommand::Rm(a) => ws_rm(opts, a).await,
        WorkspaceCommand::Diff(a) => ws_diff(opts, a).await,
        WorkspaceCommand::Log(a) => ws_log(opts, a).await,
        WorkspaceCommand::Ann(a) => match &a.cmd {
            WorkspaceAnnCommand::Get(inner) => ws_ann_get(opts, inner).await,
            WorkspaceAnnCommand::Set(inner) => ws_ann_set(opts, inner).await,
            WorkspaceAnnCommand::Del(inner) => ws_ann_del(opts, inner).await,
        },
    }
}

async fn ws_resolve(opts: &WorldOpts, args: &WorkspaceResolveArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let reference = parse_workspace_ref(&args.reference)?;
    let params = WorkspaceResolveParams {
        workspace: reference.workspace,
        version: reference.version,
    };

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let receipt = control_workspace_resolve(&mut client, &params).await?;
            let data = serde_json::to_value(receipt)?;
            return print_success(opts, data, None, vec![]);
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }

    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let mut host = create_host(store, loaded, &dirs, opts)?;
    let receipt = batch_workspace_resolve(&mut host, &params)?;
    print_success(opts, serde_json::to_value(receipt)?, None, fallback_warning(opts))
}

async fn ws_ls(opts: &WorldOpts, args: &WorkspaceLsArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    if args.reference.is_none() {
        return ws_ls_workspaces(opts, &dirs).await;
    }
    let reference = parse_workspace_ref(args.reference.as_ref().unwrap())?;
    let scope = args.scope.clone();
    let limit = args.limit.unwrap_or(0);
    let cursor = args.cursor.clone();

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let resolved = control_workspace_resolve(
                &mut client,
                &WorkspaceResolveParams {
                    workspace: reference.workspace.clone(),
                    version: reference.version,
                },
            )
            .await?;
            let root_hash = require_root_hash(&resolved)?;
            let params = WorkspaceListParams {
                root_hash,
                path: reference.path.clone(),
                scope,
                cursor,
                limit,
            };
            let receipt = control_workspace_list(&mut client, &params).await?;
            return print_success(opts, serde_json::to_value(receipt)?, None, vec![]);
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }

    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let mut host = create_host(store, loaded, &dirs, opts)?;
    let resolved = batch_workspace_resolve(
        &mut host,
        &WorkspaceResolveParams {
            workspace: reference.workspace.clone(),
            version: reference.version,
        },
    )?;
    let root_hash = require_root_hash(&resolved)?;
    let params = WorkspaceListParams {
        root_hash,
        path: reference.path.clone(),
        scope,
        cursor,
        limit,
    };
    let receipt = batch_workspace_list(&mut host, &params)?;
    print_success(opts, serde_json::to_value(receipt)?, None, fallback_warning(opts))
}

async fn ws_ls_workspaces(opts: &WorldOpts, dirs: &crate::opts::ResolvedDirs) -> Result<()> {
    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(dirs).await {
            let resp = client
                .request(&RequestEnvelope {
                    v: 1,
                    id: "cli-ws-ls".into(),
                    cmd: "state-list".into(),
                    payload: serde_json::json!({ "reducer": WORKSPACE_REDUCER }),
                })
                .await?;
            if !resp.ok {
                anyhow::bail!("workspace list failed: {:?}", resp.error);
            }
            let result = resp.result.unwrap_or_else(|| serde_json::json!({}));
            let cells = result
                .get("cells")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let names = decode_workspace_names(cells)?;
            return print_success(opts, serde_json::json!(names), None, vec![]);
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }

    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(dirs, opts)?;
    let host = create_host(store, loaded, dirs, opts)?;
    let metas = host.list_cells(WORKSPACE_REDUCER)?;
    let mut names = Vec::new();
    for meta in metas {
        if let Some(name) = decode_workspace_key(&meta.key_bytes) {
            names.push(name);
        }
    }
    names.sort();
    print_success(opts, serde_json::json!(names), None, fallback_warning(opts))
}

async fn ws_cat(opts: &WorldOpts, args: &WorkspaceCatArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let reference = parse_workspace_ref(&args.reference)?;
    let path = require_path(&reference)?;
    let range = parse_range(args.range.as_deref())?;

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let resolved = control_workspace_resolve(
                &mut client,
                &WorkspaceResolveParams {
                    workspace: reference.workspace.clone(),
                    version: reference.version,
                },
            )
            .await?;
            let root_hash = require_root_hash(&resolved)?;
            let params = WorkspaceReadBytesParams {
                root_hash,
                path: path.to_string(),
                range,
            };
            let bytes = control_workspace_read_bytes(&mut client, &params).await?;
            return output_bytes(opts, &bytes, args);
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }

    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let mut host = create_host(store, loaded, &dirs, opts)?;
    let resolved = batch_workspace_resolve(
        &mut host,
        &WorkspaceResolveParams {
            workspace: reference.workspace.clone(),
            version: reference.version,
        },
    )?;
    let root_hash = require_root_hash(&resolved)?;
    let params = WorkspaceReadBytesParams {
        root_hash,
        path: path.to_string(),
        range,
    };
    let bytes = batch_workspace_read_bytes(&mut host, &params)?;
    output_bytes(opts, &bytes, args)
}

async fn ws_stat(opts: &WorldOpts, args: &WorkspaceStatArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let reference = parse_workspace_ref(&args.reference)?;
    let path = require_path(&reference)?;

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let resolved = control_workspace_resolve(
                &mut client,
                &WorkspaceResolveParams {
                    workspace: reference.workspace.clone(),
                    version: reference.version,
                },
            )
            .await?;
            let root_hash = require_root_hash(&resolved)?;
            let params = WorkspaceReadRefParams {
                root_hash,
                path: path.to_string(),
            };
            let entry = control_workspace_read_ref(&mut client, &params)
                .await?
                .ok_or_else(|| anyhow!("path not found"))?;
            let data = serde_json::to_value(entry)?;
            return print_success(opts, data, None, vec![]);
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }

    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let mut host = create_host(store, loaded, &dirs, opts)?;
    let resolved = batch_workspace_resolve(
        &mut host,
        &WorkspaceResolveParams {
            workspace: reference.workspace.clone(),
            version: reference.version,
        },
    )?;
    let root_hash = require_root_hash(&resolved)?;
    let params = WorkspaceReadRefParams {
        root_hash,
        path: path.to_string(),
    };
    let entry = batch_workspace_read_ref(&mut host, &params)?
        .ok_or_else(|| anyhow!("path not found"))?;
    print_success(opts, serde_json::to_value(entry)?, None, fallback_warning(opts))
}

async fn ws_write(opts: &WorldOpts, args: &WorkspaceWriteArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let reference = parse_workspace_ref(&args.reference)?;
    let path = require_path(&reference)?;
    let mode = parse_mode(args.file_mode.as_deref())?;
    let data = read_write_input(args)?;
    let owner = resolve_owner(args.owner.as_deref());

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let (base_root, expected_head) = resolve_or_init_workspace_control(
                &mut client,
                &reference,
                &owner,
            )
            .await?;
            let params = WorkspaceWriteBytesParams {
                root_hash: base_root,
                path: path.to_string(),
                bytes: data,
                mode,
            };
            let receipt = control_workspace_write_bytes(&mut client, &params).await?;
            commit_workspace_control(
                &mut client,
                &reference.workspace,
                expected_head,
                &receipt.new_root_hash,
                &owner,
            )
            .await?;
            let data = serde_json::to_value(receipt)?;
            return print_success(opts, data, None, vec![]);
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }

    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let mut host = create_host(store, loaded, &dirs, opts)?;
    let (base_root, expected_head) =
        resolve_or_init_workspace_batch(&mut host, &reference, &owner)?;
    let params = WorkspaceWriteBytesParams {
        root_hash: base_root,
        path: path.to_string(),
        bytes: data,
        mode,
    };
    let receipt = batch_workspace_write_bytes(&mut host, &params)?;
    commit_workspace_batch(
        &mut host,
        &reference.workspace,
        expected_head,
        &receipt.new_root_hash,
        &owner,
    )?;
    print_success(opts, serde_json::to_value(receipt)?, None, fallback_warning(opts))
}

async fn ws_rm(opts: &WorldOpts, args: &WorkspaceRmArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let reference = parse_workspace_ref(&args.reference)?;
    let path = require_path(&reference)?;
    let owner = resolve_owner(args.owner.as_deref());

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let (base_root, expected_head) = resolve_or_init_workspace_control(
                &mut client,
                &reference,
                &owner,
            )
            .await?;
            let params = WorkspaceRemoveParams {
                root_hash: base_root,
                path: path.to_string(),
            };
            let receipt = control_workspace_remove(&mut client, &params).await?;
            commit_workspace_control(
                &mut client,
                &reference.workspace,
                expected_head,
                &receipt.new_root_hash,
                &owner,
            )
            .await?;
            let data = serde_json::to_value(receipt)?;
            return print_success(opts, data, None, vec![]);
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }

    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let mut host = create_host(store, loaded, &dirs, opts)?;
    let (base_root, expected_head) =
        resolve_or_init_workspace_batch(&mut host, &reference, &owner)?;
    let params = WorkspaceRemoveParams {
        root_hash: base_root,
        path: path.to_string(),
    };
    let receipt = batch_workspace_remove(&mut host, &params)?;
    commit_workspace_batch(
        &mut host,
        &reference.workspace,
        expected_head,
        &receipt.new_root_hash,
        &owner,
    )?;
    print_success(opts, serde_json::to_value(receipt)?, None, fallback_warning(opts))
}

async fn ws_diff(opts: &WorldOpts, args: &WorkspaceDiffArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let ref_a = parse_workspace_ref(&args.ref_a)?;
    let ref_b = parse_workspace_ref(&args.ref_b)?;
    let prefix = resolve_diff_prefix(&ref_a, &ref_b, args.prefix.as_deref())?;

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let resolved_a = control_workspace_resolve(
                &mut client,
                &WorkspaceResolveParams {
                    workspace: ref_a.workspace.clone(),
                    version: ref_a.version,
                },
            )
            .await?;
            let resolved_b = control_workspace_resolve(
                &mut client,
                &WorkspaceResolveParams {
                    workspace: ref_b.workspace.clone(),
                    version: ref_b.version,
                },
            )
            .await?;
            let params = WorkspaceDiffParams {
                root_a: require_root_hash(&resolved_a)?,
                root_b: require_root_hash(&resolved_b)?,
                prefix,
            };
            let receipt = control_workspace_diff(&mut client, &params).await?;
            return print_success(opts, serde_json::to_value(receipt)?, None, vec![]);
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }

    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let mut host = create_host(store, loaded, &dirs, opts)?;
    let resolved_a = batch_workspace_resolve(
        &mut host,
        &WorkspaceResolveParams {
            workspace: ref_a.workspace.clone(),
            version: ref_a.version,
        },
    )?;
    let resolved_b = batch_workspace_resolve(
        &mut host,
        &WorkspaceResolveParams {
            workspace: ref_b.workspace.clone(),
            version: ref_b.version,
        },
    )?;
    let params = WorkspaceDiffParams {
        root_a: require_root_hash(&resolved_a)?,
        root_b: require_root_hash(&resolved_b)?,
        prefix,
    };
    let receipt = batch_workspace_diff(&mut host, &params)?;
    print_success(opts, serde_json::to_value(receipt)?, None, fallback_warning(opts))
}

async fn ws_log(opts: &WorldOpts, args: &WorkspaceLogArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let key_bytes = workspace_key_bytes(&args.workspace)?;
    let mut warnings = Vec::new();

    let history = if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let (_meta, state_opt) = client
                .query_state_decoded(
                    "cli-ws-log",
                    WORKSPACE_REDUCER,
                    Some(&key_bytes),
                    None,
                )
                .await?;
            match state_opt {
                Some(bytes) => serde_cbor::from_slice::<WorkspaceHistory>(&bytes)
                    .context("decode workspace history")?,
                None => anyhow::bail!("workspace '{}' not found", args.workspace),
            }
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        } else {
            warnings = fallback_warning(opts);
            load_world_env(&dirs.world)?;
            let (store, loaded) = prepare_world(&dirs, opts)?;
            let host = create_host(store, loaded, &dirs, opts)?;
            read_workspace_history(&host, &args.workspace, &key_bytes)?
        }
    } else {
        warnings = fallback_warning(opts);
        load_world_env(&dirs.world)?;
        let (store, loaded) = prepare_world(&dirs, opts)?;
        let host = create_host(store, loaded, &dirs, opts)?;
        read_workspace_history(&host, &args.workspace, &key_bytes)?
    };

    let mut versions = Vec::new();
    for (version, meta) in history.versions {
        versions.push(serde_json::json!({
            "version": version,
            "root_hash": meta.root_hash,
            "owner": meta.owner,
            "created_at": meta.created_at,
        }));
    }
    let data = serde_json::json!({
        "workspace": args.workspace,
        "latest": history.latest,
        "versions": versions,
    });
    print_success(opts, data, None, warnings)
}

async fn ws_ann_get(opts: &WorldOpts, args: &WorkspaceAnnGetArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let reference = parse_workspace_ref(&args.reference)?;

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let resolved = control_workspace_resolve(
                &mut client,
                &WorkspaceResolveParams {
                    workspace: reference.workspace.clone(),
                    version: reference.version,
                },
            )
            .await?;
            let root_hash = require_root_hash(&resolved)?;
            let params = WorkspaceAnnotationsGetParams {
                root_hash,
                path: reference.path.clone(),
            };
            let receipt = control_workspace_annotations_get(&mut client, &params).await?;
            let data = serde_json::json!({ "annotations": receipt.annotations });
            return print_success(opts, data, None, vec![]);
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }

    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let mut host = create_host(store, loaded, &dirs, opts)?;
    let resolved = batch_workspace_resolve(
        &mut host,
        &WorkspaceResolveParams {
            workspace: reference.workspace.clone(),
            version: reference.version,
        },
    )?;
    let root_hash = require_root_hash(&resolved)?;
    let params = WorkspaceAnnotationsGetParams {
        root_hash,
        path: reference.path.clone(),
    };
    let receipt = batch_workspace_annotations_get(&mut host, &params)?;
    let data = serde_json::json!({ "annotations": receipt.annotations });
    print_success(opts, data, None, fallback_warning(opts))
}

async fn ws_ann_set(opts: &WorldOpts, args: &WorkspaceAnnSetArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let reference = parse_workspace_ref(&args.reference)?;
    let owner = resolve_owner(args.owner.as_deref());
    let patch = parse_annotation_pairs(&args.entries)?;

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let (base_root, expected_head) = resolve_or_init_workspace_control(
                &mut client,
                &reference,
                &owner,
            )
            .await?;
            let params = WorkspaceAnnotationsSetParams {
                root_hash: base_root,
                path: reference.path.clone(),
                annotations_patch: WorkspaceAnnotationsPatch(patch),
            };
            let receipt = control_workspace_annotations_set(&mut client, &params).await?;
            commit_workspace_control(
                &mut client,
                &reference.workspace,
                expected_head,
                &receipt.new_root_hash,
                &owner,
            )
            .await?;
            let data = serde_json::to_value(receipt)?;
            return print_success(opts, data, None, vec![]);
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }

    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let mut host = create_host(store, loaded, &dirs, opts)?;
    let (base_root, expected_head) =
        resolve_or_init_workspace_batch(&mut host, &reference, &owner)?;
    let params = WorkspaceAnnotationsSetParams {
        root_hash: base_root,
        path: reference.path.clone(),
        annotations_patch: WorkspaceAnnotationsPatch(patch),
    };
    let receipt = batch_workspace_annotations_set(&mut host, &params)?;
    commit_workspace_batch(
        &mut host,
        &reference.workspace,
        expected_head,
        &receipt.new_root_hash,
        &owner,
    )?;
    print_success(opts, serde_json::to_value(receipt)?, None, fallback_warning(opts))
}

async fn ws_ann_del(opts: &WorldOpts, args: &WorkspaceAnnDelArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let reference = parse_workspace_ref(&args.reference)?;
    let owner = resolve_owner(args.owner.as_deref());
    let patch = parse_annotation_deletes(&args.keys)?;

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let (base_root, expected_head) = resolve_or_init_workspace_control(
                &mut client,
                &reference,
                &owner,
            )
            .await?;
            let params = WorkspaceAnnotationsSetParams {
                root_hash: base_root,
                path: reference.path.clone(),
                annotations_patch: WorkspaceAnnotationsPatch(patch),
            };
            let receipt = control_workspace_annotations_set(&mut client, &params).await?;
            commit_workspace_control(
                &mut client,
                &reference.workspace,
                expected_head,
                &receipt.new_root_hash,
                &owner,
            )
            .await?;
            let data = serde_json::to_value(receipt)?;
            return print_success(opts, data, None, vec![]);
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }

    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let mut host = create_host(store, loaded, &dirs, opts)?;
    let (base_root, expected_head) =
        resolve_or_init_workspace_batch(&mut host, &reference, &owner)?;
    let params = WorkspaceAnnotationsSetParams {
        root_hash: base_root,
        path: reference.path.clone(),
        annotations_patch: WorkspaceAnnotationsPatch(patch),
    };
    let receipt = batch_workspace_annotations_set(&mut host, &params)?;
    commit_workspace_batch(
        &mut host,
        &reference.workspace,
        expected_head,
        &receipt.new_root_hash,
        &owner,
    )?;
    print_success(opts, serde_json::to_value(receipt)?, None, fallback_warning(opts))
}

async fn control_workspace_resolve(
    client: &mut ControlClient,
    params: &WorkspaceResolveParams,
) -> Result<WorkspaceResolveReceipt> {
    control_call(client, "workspace-resolve", params).await
}

async fn control_workspace_list(
    client: &mut ControlClient,
    params: &WorkspaceListParams,
) -> Result<WorkspaceListReceipt> {
    control_call(client, "workspace-list", params).await
}

async fn control_workspace_read_ref(
    client: &mut ControlClient,
    params: &WorkspaceReadRefParams,
) -> Result<Option<WorkspaceRefEntry>> {
    control_call(client, "workspace-read-ref", params).await
}

async fn control_workspace_read_bytes(
    client: &mut ControlClient,
    params: &WorkspaceReadBytesParams,
) -> Result<Vec<u8>> {
    let payload = serde_json::to_value(params)?;
    let resp = client
        .request(&RequestEnvelope {
            v: 1,
            id: "cli-ws-read-bytes".into(),
            cmd: "workspace-read-bytes".into(),
            payload,
        })
        .await?;
    if !resp.ok {
        anyhow::bail!(
            "workspace read-bytes failed: {:?}",
            resp.error.map(|e| e.message)
        );
    }
    let result = resp.result.unwrap_or_else(|| serde_json::json!({}));
    let data_b64 = result
        .get("data_b64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("workspace read-bytes missing data_b64"))?;
    base64::engine::general_purpose::STANDARD
        .decode(data_b64)
        .context("decode data_b64")
}

async fn control_workspace_write_bytes(
    client: &mut ControlClient,
    params: &WorkspaceWriteBytesParams,
) -> Result<WorkspaceWriteBytesReceipt> {
    #[derive(Serialize)]
    struct Payload<'a> {
        root_hash: &'a String,
        path: &'a String,
        bytes_b64: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mode: Option<u64>,
    }
    let payload = Payload {
        root_hash: &params.root_hash,
        path: &params.path,
        bytes_b64: base64::engine::general_purpose::STANDARD.encode(&params.bytes),
        mode: params.mode,
    };
    control_call(client, "workspace-write-bytes", &payload).await
}

async fn control_workspace_remove(
    client: &mut ControlClient,
    params: &WorkspaceRemoveParams,
) -> Result<WorkspaceRemoveReceipt> {
    control_call(client, "workspace-remove", params).await
}

async fn control_workspace_diff(
    client: &mut ControlClient,
    params: &WorkspaceDiffParams,
) -> Result<WorkspaceDiffReceipt> {
    control_call(client, "workspace-diff", params).await
}

async fn control_workspace_annotations_get(
    client: &mut ControlClient,
    params: &WorkspaceAnnotationsGetParams,
) -> Result<WorkspaceAnnotationsGetReceipt> {
    control_call(client, "workspace-annotations-get", params).await
}

async fn control_workspace_annotations_set(
    client: &mut ControlClient,
    params: &WorkspaceAnnotationsSetParams,
) -> Result<WorkspaceAnnotationsSetReceipt> {
    control_call(client, "workspace-annotations-set", params).await
}

async fn control_workspace_empty_root(
    client: &mut ControlClient,
    workspace: &str,
) -> Result<String> {
    let params = WorkspaceEmptyRootParams {
        workspace: workspace.to_string(),
    };
    let receipt: WorkspaceEmptyRootReceipt =
        control_call(client, "workspace-empty-root", &params).await?;
    Ok(receipt.root_hash)
}

fn batch_workspace_resolve(
    host: &mut WorldHost<FsStore>,
    params: &WorkspaceResolveParams,
) -> Result<WorkspaceResolveReceipt> {
    handle_internal(host, EffectKind::workspace_resolve(), params, "workspace.resolve")
}

fn batch_workspace_empty_root(
    host: &mut WorldHost<FsStore>,
    workspace: &str,
) -> Result<String> {
    let params = WorkspaceEmptyRootParams {
        workspace: workspace.to_string(),
    };
    let receipt: WorkspaceEmptyRootReceipt = handle_internal(
        host,
        EffectKind::workspace_empty_root(),
        &params,
        "workspace.empty_root",
    )?;
    Ok(receipt.root_hash)
}

fn batch_workspace_list(
    host: &mut WorldHost<FsStore>,
    params: &WorkspaceListParams,
) -> Result<WorkspaceListReceipt> {
    handle_internal(host, EffectKind::workspace_list(), params, "workspace.list")
}

fn batch_workspace_read_ref(
    host: &mut WorldHost<FsStore>,
    params: &WorkspaceReadRefParams,
) -> Result<Option<WorkspaceRefEntry>> {
    handle_internal(host, EffectKind::workspace_read_ref(), params, "workspace.read_ref")
}

fn batch_workspace_read_bytes(
    host: &mut WorldHost<FsStore>,
    params: &WorkspaceReadBytesParams,
) -> Result<Vec<u8>> {
    handle_internal(host, EffectKind::workspace_read_bytes(), params, "workspace.read_bytes")
}

fn batch_workspace_write_bytes(
    host: &mut WorldHost<FsStore>,
    params: &WorkspaceWriteBytesParams,
) -> Result<WorkspaceWriteBytesReceipt> {
    handle_internal(host, EffectKind::workspace_write_bytes(), params, "workspace.write_bytes")
}

fn batch_workspace_remove(
    host: &mut WorldHost<FsStore>,
    params: &WorkspaceRemoveParams,
) -> Result<WorkspaceRemoveReceipt> {
    handle_internal(host, EffectKind::workspace_remove(), params, "workspace.remove")
}

fn batch_workspace_diff(
    host: &mut WorldHost<FsStore>,
    params: &WorkspaceDiffParams,
) -> Result<WorkspaceDiffReceipt> {
    handle_internal(host, EffectKind::workspace_diff(), params, "workspace.diff")
}

fn batch_workspace_annotations_get(
    host: &mut WorldHost<FsStore>,
    params: &WorkspaceAnnotationsGetParams,
) -> Result<WorkspaceAnnotationsGetReceipt> {
    handle_internal(
        host,
        EffectKind::workspace_annotations_get(),
        params,
        "workspace.annotations_get",
    )
}

fn batch_workspace_annotations_set(
    host: &mut WorldHost<FsStore>,
    params: &WorkspaceAnnotationsSetParams,
) -> Result<WorkspaceAnnotationsSetReceipt> {
    handle_internal(
        host,
        EffectKind::workspace_annotations_set(),
        params,
        "workspace.annotations_set",
    )
}

fn handle_internal<T: serde::de::DeserializeOwned, P: Serialize>(
    host: &mut WorldHost<FsStore>,
    kind: EffectKind,
    params: &P,
    label: &str,
) -> Result<T> {
    let intent = IntentBuilder::new(kind, WORKSPACE_CAP, params)
        .build()
        .map_err(|e| anyhow!("encode {label} params: {e}"))?;
    let receipt = host
        .kernel_mut()
        .handle_internal_intent(&intent)?
        .ok_or_else(|| anyhow!("{label} not handled as internal effect"))?;
    if receipt.status != ReceiptStatus::Ok {
        if let Some(message) = decode_internal_error_message(&receipt.payload_cbor) {
            anyhow::bail!("{label} failed: {message}");
        }
        anyhow::bail!("{label} failed");
    }
    receipt
        .payload::<T>()
        .map_err(|e| anyhow!("decode {label} receipt: {e}"))
}

async fn control_call<T: serde::de::DeserializeOwned, P: Serialize>(
    client: &mut ControlClient,
    cmd: &str,
    payload: &P,
) -> Result<T> {
    let payload = serde_json::to_value(payload)?;
    let resp = client
        .request(&RequestEnvelope {
            v: 1,
            id: format!("cli-ws-{cmd}"),
            cmd: cmd.to_string(),
            payload,
        })
        .await?;
    if !resp.ok {
        anyhow::bail!(
            "{cmd} failed: {:?}",
            resp.error.map(|e| e.message)
        );
    }
    let result = resp.result.unwrap_or(serde_json::Value::Null);
    serde_json::from_value(result).context("decode control response")
}

fn parse_workspace_ref(input: &str) -> Result<WorkspaceRef> {
    let input = input.trim_end_matches('/');
    if input.is_empty() {
        anyhow::bail!("workspace ref is required");
    }
    if input.starts_with('/') {
        anyhow::bail!("workspace ref cannot start with '/'");
    }
    let (head, path) = match input.split_once('/') {
        Some((head, path)) => {
            if path.is_empty() || path.starts_with('/') {
                anyhow::bail!("invalid workspace path");
            }
            (head, Some(path.to_string()))
        }
        None => (input, None),
    };
    let (workspace, version) = match head.split_once('@') {
        Some((name, version)) => {
            if name.is_empty() || version.is_empty() {
                anyhow::bail!("invalid workspace ref");
            }
            let version = version
                .parse::<u64>()
                .map_err(|_| anyhow!("invalid workspace version"))?;
            (name.to_string(), Some(version))
        }
        None => (head.to_string(), None),
    };
    Ok(WorkspaceRef {
        workspace,
        version,
        path,
    })
}

fn require_path(reference: &WorkspaceRef) -> Result<&str> {
    reference
        .path
        .as_deref()
        .ok_or_else(|| anyhow!("path required in workspace ref"))
}

fn require_root_hash(resolve: &WorkspaceResolveReceipt) -> Result<String> {
    if !resolve.exists {
        anyhow::bail!("workspace does not exist");
    }
    resolve
        .root_hash
        .clone()
        .ok_or_else(|| anyhow!("workspace root hash missing"))
}

fn parse_range(input: Option<&str>) -> Result<Option<WorkspaceReadBytesRange>> {
    let Some(raw) = input else {
        return Ok(None);
    };
    let Some((start, end)) = raw.split_once(':') else {
        anyhow::bail!("range must be START:END");
    };
    let start = start
        .parse::<u64>()
        .map_err(|_| anyhow!("invalid range start"))?;
    let end = end.parse::<u64>().map_err(|_| anyhow!("invalid range end"))?;
    Ok(Some(WorkspaceReadBytesRange { start, end }))
}

fn parse_mode(input: Option<&str>) -> Result<Option<u64>> {
    let Some(raw) = input else {
        return Ok(None);
    };
    let trimmed = raw.trim_start_matches('0');
    let value = match trimmed {
        "644" => 0o644,
        "755" => 0o755,
        _ => anyhow::bail!("mode must be 644 or 755"),
    };
    Ok(Some(value))
}

fn read_write_input(args: &WorkspaceWriteArgs) -> Result<Vec<u8>> {
    if let Some(text) = &args.text_in {
        return Ok(text.as_bytes().to_vec());
    }
    if let Some(json) = &args.json_in {
        let json_str = parse_input_value(json)?;
        let value: JsonValue = serde_json::from_str(&json_str).context("parse --json-in")?;
        return serde_json::to_vec(&value).context("encode json");
    }
    let input = args.input.as_deref().unwrap_or_default();
    if input == "@-" {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .context("read stdin")?;
        return Ok(buf);
    }
    fs::read(input).with_context(|| format!("read input file {}", input))
}

fn output_bytes(opts: &WorldOpts, bytes: &[u8], args: &WorkspaceCatArgs) -> Result<()> {
    if let Some(out) = &args.out {
        fs::write(out, bytes)?;
        return print_success(
            opts,
            serde_json::json!({ "written": out, "bytes": bytes.len() }),
            None,
            vec![],
        );
    }
    if args.raw {
        use std::io::Write;
        let mut stdout = std::io::stdout();
        stdout.write_all(bytes)?;
        stdout.flush()?;
        return Ok(());
    }

    if let Ok(text) = std::str::from_utf8(bytes) {
        if let Ok(json) = serde_json::from_str::<JsonValue>(text) {
            return print_success(opts, json, None, vec![]);
        }
        return print_success(opts, JsonValue::String(text.to_string()), None, vec![]);
    }

    if let Ok(value) = serde_cbor::from_slice::<JsonValue>(bytes) {
        return print_success(opts, value, None, vec![]);
    }

    anyhow::bail!("binary content; use --raw or --out")
}

fn decode_workspace_names(cells: Vec<JsonValue>) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for cell in cells {
        let key_b64 = cell
            .get("key_b64")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode(key_b64)
            .unwrap_or_default();
        if let Some(name) = decode_workspace_key(&key_bytes) {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

fn decode_workspace_key(bytes: &[u8]) -> Option<String> {
    serde_cbor::from_slice::<String>(bytes).ok()
}

fn normalize_path_arg(input: &str) -> Result<String> {
    let trimmed = input.trim_end_matches('/');
    if trimmed.is_empty() || trimmed.starts_with('/') {
        anyhow::bail!("invalid workspace path");
    }
    Ok(trimmed.to_string())
}

fn decode_internal_error_message(payload: &[u8]) -> Option<String> {
    if payload.is_empty() {
        return None;
    }
    serde_cbor::from_slice::<String>(payload).ok()
}

fn resolve_diff_prefix(
    ref_a: &WorkspaceRef,
    ref_b: &WorkspaceRef,
    explicit: Option<&str>,
) -> Result<Option<String>> {
    if explicit.is_some() && (ref_a.path.is_some() || ref_b.path.is_some()) {
        anyhow::bail!("--prefix cannot be combined with ref paths");
    }
    let path_a = ref_a.path.as_deref();
    let path_b = ref_b.path.as_deref();
    if let (Some(a), Some(b)) = (path_a, path_b) {
        if a != b {
            anyhow::bail!("diff refs must share the same path");
        }
    }
    let explicit = explicit
        .map(normalize_path_arg)
        .transpose()?;
    Ok(explicit
        .or_else(|| path_a.map(|s| s.to_string()))
        .or_else(|| path_b.map(|s| s.to_string())))
}

async fn resolve_or_init_workspace_control(
    client: &mut ControlClient,
    reference: &WorkspaceRef,
    owner: &str,
) -> Result<(String, Option<u64>)> {
    let resolved = control_workspace_resolve(
        client,
        &WorkspaceResolveParams {
            workspace: reference.workspace.clone(),
            version: reference.version,
        },
    )
    .await?;
    if resolved.exists {
        let root_hash = require_root_hash(&resolved)?;
        return Ok((root_hash, resolved.resolved_version));
    }
    let root_hash = control_workspace_empty_root(client, &reference.workspace).await?;
    commit_workspace_control(client, &reference.workspace, None, &root_hash, owner).await?;
    Ok((root_hash, Some(1)))
}

fn resolve_or_init_workspace_batch(
    host: &mut WorldHost<FsStore>,
    reference: &WorkspaceRef,
    owner: &str,
) -> Result<(String, Option<u64>)> {
    let resolved = batch_workspace_resolve(
        host,
        &WorkspaceResolveParams {
            workspace: reference.workspace.clone(),
            version: reference.version,
        },
    )?;
    if resolved.exists {
        let root_hash = require_root_hash(&resolved)?;
        return Ok((root_hash, resolved.resolved_version));
    }
    let root_hash = batch_workspace_empty_root(host, &reference.workspace)?;
    commit_workspace_batch(host, &reference.workspace, None, &root_hash, owner)?;
    Ok((root_hash, Some(1)))
}

async fn commit_workspace_control(
    client: &mut ControlClient,
    workspace: &str,
    expected_head: Option<u64>,
    root_hash: &str,
    owner: &str,
) -> Result<()> {
    let payload = build_workspace_commit(workspace, expected_head, root_hash, owner)?;
    let resp = client
        .send_event("cli-ws-commit", WORKSPACE_EVENT, None, &payload)
        .await?;
    if !resp.ok {
        anyhow::bail!(
            "workspace commit failed: {:?}",
            resp.error.map(|e| e.message)
        );
    }
    Ok(())
}

fn commit_workspace_batch(
    host: &mut WorldHost<FsStore>,
    workspace: &str,
    expected_head: Option<u64>,
    root_hash: &str,
    owner: &str,
) -> Result<()> {
    let payload = build_workspace_commit(workspace, expected_head, root_hash, owner)?;
    host.kernel_mut()
        .submit_domain_event_result(WORKSPACE_EVENT, payload)
        .map_err(|e| anyhow!("workspace commit failed: {e}"))
}

fn build_workspace_commit(
    workspace: &str,
    expected_head: Option<u64>,
    root_hash: &str,
    owner: &str,
) -> Result<Vec<u8>> {
    let created_at = now_ns();
    let event = WorkspaceCommit {
        workspace: workspace.to_string(),
        expected_head,
        meta: WorkspaceCommitMeta {
            root_hash: root_hash.to_string(),
            owner: owner.to_string(),
            created_at,
        },
    };
    serde_cbor::to_vec(&event).context("encode workspace commit")
}

fn resolve_owner(owner: Option<&str>) -> String {
    if let Some(value) = owner.and_then(|v| normalize_owner(v)) {
        return value;
    }
    if let Ok(env_owner) = env::var("AOS_OWNER") {
        if let Some(value) = normalize_owner(&env_owner) {
            return value;
        }
    }
    for key in ["USER", "LOGNAME", "USERNAME"] {
        if let Ok(value) = env::var(key) {
            if let Some(value) = normalize_owner(&value) {
                return value;
            }
        }
    }
    "unknown".into()
}

fn normalize_owner(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn parse_annotation_pairs(entries: &[String]) -> Result<BTreeMap<String, Option<String>>> {
    if entries.is_empty() {
        anyhow::bail!("at least one annotation entry is required");
    }
    let mut map = BTreeMap::new();
    for entry in entries {
        let (key, value) = entry
            .split_once('=')
            .ok_or_else(|| anyhow!("invalid annotation entry '{}'", entry))?;
        let key = normalize_hash_ref(key)?;
        let value = normalize_hash_ref(value)?;
        map.insert(key, Some(value));
    }
    Ok(map)
}

fn parse_annotation_deletes(keys: &[String]) -> Result<BTreeMap<String, Option<String>>> {
    if keys.is_empty() {
        anyhow::bail!("at least one annotation key is required");
    }
    let mut map = BTreeMap::new();
    for key in keys {
        let key = normalize_hash_ref(key)?;
        map.insert(key, None);
    }
    Ok(map)
}

fn normalize_hash_ref(input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.starts_with("sha256:") {
        aos_cbor::Hash::from_hex_str(trimmed)
            .map_err(|_| anyhow!("invalid hash ref '{}'", input))?;
        return Ok(trimmed.to_string());
    }
    if trimmed.len() == 64 && hex::decode(trimmed).is_ok() {
        return Ok(format!("sha256:{trimmed}"));
    }
    anyhow::bail!("invalid hash ref '{}'", input)
}

fn fallback_warning(opts: &WorldOpts) -> Vec<String> {
    if opts.quiet {
        vec![]
    } else {
        vec!["daemon unavailable; using batch mode".into()]
    }
}

fn workspace_key_bytes(workspace: &str) -> Result<Vec<u8>> {
    aos_cbor::to_canonical_cbor(&workspace.to_string()).context("encode workspace key")
}

fn read_workspace_history(
    host: &WorldHost<FsStore>,
    workspace: &str,
    key_bytes: &[u8],
) -> Result<WorkspaceHistory> {
    let read = host
        .query_state(WORKSPACE_REDUCER, Some(key_bytes), aos_kernel::Consistency::Head)
        .ok_or_else(|| anyhow!("workspace '{}' not found", workspace))?;
    let bytes = read
        .value
        .ok_or_else(|| anyhow!("workspace '{}' not found", workspace))?;
    serde_cbor::from_slice::<WorkspaceHistory>(&bytes).context("decode workspace history")
}
