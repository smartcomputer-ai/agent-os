use std::path::PathBuf;

use anyhow::{Result, anyhow};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use crate::GlobalOpts;
use crate::authoring::{
    load_sync_entries, resolve_local_dirs, sync_workspace_pull, sync_workspace_push,
};
use crate::client::ApiClient;
use crate::output::{OutputOpts, print_success};
use crate::workspace::parse_workspace_ref;

use super::common::{
    list_workspace_names, print_workspace_cat, resolve_selected_world, resolve_target,
    resolve_workspace_ref, universe_query_for_world,
};

#[derive(Args, Debug)]
#[command(about = "Inspect and synchronize hosted workspaces")]
pub(crate) struct WorkspaceArgs {
    #[command(subcommand)]
    cmd: WorkspaceCommand,
}

#[derive(Subcommand, Debug)]
enum WorkspaceCommand {
    /// Resolve a workspace ref to a concrete root and version.
    Resolve(WorkspaceResolveArgs),
    /// List workspace names or entries under one workspace ref.
    Ls(WorkspaceLsArgs),
    /// Show one workspace entry.
    Stat(WorkspaceStatArgs),
    /// Read one workspace file.
    Cat(WorkspaceCatArgs),
    /// Read annotations for one workspace path.
    Ann(WorkspaceAnnArgs),
    /// Diff two workspace refs.
    Diff(WorkspaceDiffArgs),
    /// Push local files into hosted workspace roots and commit them.
    Push(WorkspacePushArgs),
    /// Pull hosted workspace files into the local filesystem.
    Pull(WorkspacePullArgs),
}

#[derive(Args, Debug)]
struct WorkspaceResolveArgs {
    /// Workspace ref in `<workspace>[@<version>][/path]` form.
    reference: String,
}

#[derive(Args, Debug)]
struct WorkspaceLsArgs {
    /// Workspace ref in `<workspace>[@<version>][/path]` form. Omit to list workspace names.
    reference: Option<String>,
    /// Listing scope: `dir` or `subtree`.
    #[arg(long)]
    scope: Option<String>,
    /// Maximum number of entries to return.
    #[arg(long)]
    limit: Option<u64>,
}

#[derive(Args, Debug)]
struct WorkspaceStatArgs {
    /// Workspace ref that includes a path.
    reference: String,
}

#[derive(Args, Debug)]
struct WorkspaceCatArgs {
    /// Workspace ref that includes a file path.
    reference: String,
    /// Write the file bytes to a local path instead of stdout.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Stream raw bytes to stdout without text decoding.
    #[arg(long)]
    raw: bool,
}

#[derive(Args, Debug)]
struct WorkspaceAnnArgs {
    /// Workspace ref whose annotations should be read.
    reference: String,
}

#[derive(Args, Debug)]
struct WorkspaceDiffArgs {
    /// First workspace ref to compare.
    ref_a: String,
    /// Second workspace ref to compare.
    ref_b: String,
    /// Optional path prefix to limit the diff.
    #[arg(long)]
    prefix: Option<String>,
}

#[derive(Args, Debug)]
struct WorkspacePushArgs {
    /// Local world root that contains `aos.sync.json`.
    #[arg(long)]
    local_root: Option<PathBuf>,
    /// Explicit sync map path. Defaults to `aos.sync.json`.
    #[arg(long)]
    map: Option<PathBuf>,
    /// Local directory to push when syncing one workspace explicitly.
    dir: Option<PathBuf>,
    /// Workspace ref to push when syncing one workspace explicitly.
    reference: Option<String>,
    /// Remove remote files that do not exist locally.
    #[arg(long)]
    prune: bool,
    /// Commit message stored as a workspace annotation.
    #[arg(long)]
    message: Option<String>,
}

#[derive(Args, Debug)]
struct WorkspacePullArgs {
    /// Local world root that contains `aos.sync.json`.
    #[arg(long)]
    local_root: Option<PathBuf>,
    /// Explicit sync map path. Defaults to `aos.sync.json`.
    #[arg(long)]
    map: Option<PathBuf>,
    /// Workspace ref to pull when syncing one workspace explicitly.
    reference: Option<String>,
    /// Local destination directory when syncing one workspace explicitly.
    dir: Option<PathBuf>,
    /// Remove local files that do not exist remotely.
    #[arg(long)]
    prune: bool,
}

pub(crate) async fn handle(
    global: &GlobalOpts,
    output: OutputOpts,
    args: WorkspaceArgs,
) -> Result<()> {
    let target = resolve_target(global)?;
    let client = ApiClient::new(&target)?;
    let world = resolve_selected_world(&target)?;
    match args.cmd {
        WorkspaceCommand::Resolve(args) => {
            let parsed = parse_workspace_ref(&args.reference)?;
            let data = resolve_workspace_ref(&client, &world, &parsed).await?;
            print_success(output, data, None, vec![])
        }
        WorkspaceCommand::Ls(args) => {
            let Some(reference) = args.reference.as_deref() else {
                let data = list_workspace_names(&client, &world, args.limit).await?;
                return print_success(output, data, None, vec![]);
            };
            let parsed = parse_workspace_ref(reference)?;
            let resolution = resolve_workspace_ref(&client, &world, &parsed).await?;
            let root_hash = resolution
                .get("root_hash")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("workspace '{}' does not exist", reference))?;
            let data = client
                .get_json(&format!("/v1/workspace/roots/{root_hash}/entries"), &{
                    let mut query = universe_query_for_world(&client, &world).await?;
                    query.extend([
                        ("path", parsed.path),
                        ("scope", args.scope),
                        ("limit", args.limit.map(|value| value.to_string())),
                    ]);
                    query
                })
                .await?;
            print_success(output, data, Some(resolution), vec![])
        }
        WorkspaceCommand::Stat(args) => {
            let parsed = parse_workspace_ref(&args.reference)?;
            let resolution = resolve_workspace_ref(&client, &world, &parsed).await?;
            let root_hash = resolution
                .get("root_hash")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("workspace '{}' does not exist", args.reference))?;
            let path = parsed
                .path
                .ok_or_else(|| anyhow!("workspace stat requires a path"))?;
            let data = client
                .get_json(&format!("/v1/workspace/roots/{root_hash}/entry"), &{
                    let mut query = universe_query_for_world(&client, &world).await?;
                    query.push(("path", Some(path)));
                    query
                })
                .await?;
            print_success(output, data, Some(resolution), vec![])
        }
        WorkspaceCommand::Cat(args) => {
            let parsed = parse_workspace_ref(&args.reference)?;
            let resolution = resolve_workspace_ref(&client, &world, &parsed).await?;
            let root_hash = resolution
                .get("root_hash")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("workspace '{}' does not exist", args.reference))?;
            let path = parsed
                .path
                .ok_or_else(|| anyhow!("workspace cat requires a path"))?;
            let bytes = client
                .get_bytes(&format!("/v1/workspace/roots/{root_hash}/bytes"), &{
                    let mut query = universe_query_for_world(&client, &world).await?;
                    query.push(("path", Some(path)));
                    query
                })
                .await?;
            print_workspace_cat(
                output,
                &bytes,
                args.out.as_deref(),
                args.raw,
                Some(resolution),
                vec![],
            )
        }
        WorkspaceCommand::Ann(args) => {
            let parsed = parse_workspace_ref(&args.reference)?;
            let resolution = resolve_workspace_ref(&client, &world, &parsed).await?;
            let root_hash = resolution
                .get("root_hash")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("workspace '{}' does not exist", args.reference))?;
            let data = client
                .get_json(&format!("/v1/workspace/roots/{root_hash}/annotations"), &{
                    let mut query = universe_query_for_world(&client, &world).await?;
                    query.push(("path", parsed.path));
                    query
                })
                .await?;
            print_success(output, data, Some(resolution), vec![])
        }
        WorkspaceCommand::Diff(args) => {
            let a = parse_workspace_ref(&args.ref_a)?;
            let b = parse_workspace_ref(&args.ref_b)?;
            let a_resolution = resolve_workspace_ref(&client, &world, &a).await?;
            let b_resolution = resolve_workspace_ref(&client, &world, &b).await?;
            let root_a = a_resolution
                .get("root_hash")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("workspace '{}' does not exist", args.ref_a))?;
            let root_b = b_resolution
                .get("root_hash")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("workspace '{}' does not exist", args.ref_b))?;
            let data = client
                .post_json(
                    &format!(
                        "/v1/workspace/diffs?universe_id={}",
                        universe_query_for_world(&client, &world)
                            .await?
                            .into_iter()
                            .next()
                            .and_then(|(_, value)| value)
                            .ok_or_else(|| anyhow!("world universe missing"))?
                    ),
                    &json!({
                        "root_a": root_a,
                        "root_b": root_b,
                        "prefix": args.prefix,
                    }),
                )
                .await?;
            print_success(
                output,
                data,
                Some(json!({ "a": a_resolution, "b": b_resolution })),
                vec![],
            )
        }
        WorkspaceCommand::Push(args) => {
            let dirs = resolve_local_dirs(args.local_root.as_deref())?;
            let (_, _, entries) = load_sync_entries(
                &dirs.root,
                args.map.as_deref(),
                args.reference.as_deref(),
                args.dir.as_deref(),
            )?;
            let mut results = Vec::new();
            for entry in &entries {
                results.push(
                    sync_workspace_push(
                        &client,
                        &world,
                        entry,
                        args.prune,
                        args.message.as_deref(),
                    )
                    .await?,
                );
            }
            print_success(output, Value::Array(results), None, vec![])
        }
        WorkspaceCommand::Pull(args) => {
            let dirs = resolve_local_dirs(args.local_root.as_deref())?;
            let (_, _, entries) = load_sync_entries(
                &dirs.root,
                args.map.as_deref(),
                args.reference.as_deref(),
                args.dir.as_deref(),
            )?;
            let mut results = Vec::new();
            for entry in &entries {
                results.push(sync_workspace_pull(&client, &world, entry, args.prune).await?);
            }
            print_success(output, Value::Array(results), None, vec![])
        }
    }
}
