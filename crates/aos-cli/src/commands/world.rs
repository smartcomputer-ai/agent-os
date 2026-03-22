use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use aos_air_types::{AirNode, DefSecret, Manifest, ModuleKind, SecretEntry};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effect_types::{
    GovApplyReceipt, GovApproveParams, GovApproveReceipt, GovDecision, GovPatchInput,
    GovProposeParams, GovProposeReceipt, GovShadowReceipt, HashRef,
};
use aos_node::{SecretBindingRecord, SecretBindingStatus};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use clap::{Args, Subcommand};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::GlobalOpts;
use crate::authoring::{
    build_bundle_from_world, build_patch, fetch_remote_manifest, resolve_local_dirs,
    sync_hosted_secrets, upload_bundle, upload_patch_bytes, upload_patch_json,
};
use crate::client::ApiClient;
use crate::config::{ConfigPaths, load_config, save_config};
use crate::output::{OutputOpts, print_success, print_verbose};
use crate::render::{
    print_journal_entries, print_state_cells, print_state_get, print_trace, print_trace_summary,
};

use super::common::{
    decode_command_payload, default_approver, encode_path_segment, ensure_approved, fetch_command,
    is_terminal_state, resolve_target, resolve_world_arg_or_selected, resolve_world_selector,
    universe_query_for_world, wait_for_command,
};

#[derive(Args, Debug)]
#[command(about = "Manage worlds, world state, governance, and events")]
pub(crate) struct WorldArgs {
    #[command(subcommand)]
    cmd: WorldCommand,
}

#[derive(Subcommand, Debug)]
enum WorldCommand {
    /// List visible worlds.
    Ls,
    /// Show one world by ID or the selected default.
    Get(WorldGetArgs),
    /// Create a hosted world, upload a local bundle, or fork from an existing world.
    Create(WorldCreateArgs),
    /// Show current runtime and scheduling status for a world.
    Status(WorldGetArgs),
    /// Fetch the current manifest for a world.
    Manifest(WorldGetArgs),
    /// Query definition objects in the current manifest.
    Defs(WorldDefsArgs),
    /// Query workflow state cells.
    State(WorldStateArgs),
    /// Query journal metadata and entries.
    Journal(WorldJournalArgs),
    /// Inject a domain event into the selected world.
    Send(WorldSendArgs),
    /// Inspect asynchronous command status.
    Cmd(WorldCmdArgs),
    /// Submit governance commands.
    Gov(WorldGovArgs),
    /// Inspect world traces.
    Trace(WorldTraceArgs),
    /// Build and submit a manifest patch from a local authored world.
    Patch(WorldPatchArgs),
}

#[derive(Args, Debug)]
struct WorldGetArgs {
    /// World ID. Defaults to the selected world.
    selector: Option<String>,
}

#[derive(Args, Debug)]
struct WorldCreateArgs {
    /// Local world root to build and upload before creation.
    #[arg(long)]
    local_root: Option<PathBuf>,
    /// Force a fresh workflow build instead of reusing cache.
    #[arg(long)]
    force_build: bool,
    /// Existing manifest hash to create from when not using `--local-root`.
    #[arg(long)]
    manifest_hash: Option<String>,
    /// Existing world ID to fork from instead of creating from a manifest.
    #[arg(long)]
    from_world: Option<String>,
    /// Upload the local bundle and stop after printing the manifest hash.
    #[arg(long)]
    upload_only: bool,
    /// Make the created world the selected world on the current CLI profile.
    #[arg(long)]
    select: bool,
    /// Sync configured secrets from `aos.sync.json` before create. On local targets this is a
    /// compatibility no-op; local secrets resolve from env/`.env` at world load.
    #[arg(long)]
    sync_secrets: bool,
    /// Explicit world ID to create.
    #[arg(long)]
    world_id: Option<String>,
    /// Universe to assign at world creation time.
    #[arg(long)]
    universe_id: Option<String>,
    /// Specific snapshot ref to fork from.
    #[arg(long)]
    snapshot_ref: Option<String>,
    /// Specific snapshot height to fork from.
    #[arg(long)]
    snapshot_height: Option<u64>,
}

#[derive(Args, Debug)]
#[command(about = "Query definition objects in the current world manifest")]
struct WorldDefsArgs {
    #[command(subcommand)]
    cmd: WorldDefsCommand,
}

#[derive(Subcommand, Debug)]
enum WorldDefsCommand {
    /// List defs in the current manifest.
    Ls(WorldDefsListArgs),
    /// Fetch one def by kind and name.
    Get(WorldDefsGetArgs),
}

#[derive(Args, Debug)]
struct WorldDefsListArgs {
    /// Comma-separated def kinds to include: `schema`, `module`, `cap`, `effect`, or `policy`.
    #[arg(long)]
    kinds: Option<String>,
    /// Filter def names by prefix.
    #[arg(long)]
    prefix: Option<String>,
}

#[derive(Args, Debug)]
struct WorldDefsGetArgs {
    /// Def kind: `schema`, `module`, `cap`, `effect`, or `policy`.
    kind: String,
    /// Def name to fetch.
    name: String,
}

#[derive(Args, Debug)]
#[command(about = "Query workflow state cells in the current world")]
struct WorldStateArgs {
    #[command(subcommand)]
    cmd: WorldStateCommand,
}

#[derive(Subcommand, Debug)]
enum WorldStateCommand {
    /// List workflow modules that expose state in the current world.
    Ls,
    /// Read one workflow state value by key.
    Get(WorldStateGetArgs),
    /// List cells for one workflow.
    Cells(WorldStateCellsArgs),
}

#[derive(Args, Debug)]
struct WorldStateGetArgs {
    /// Workflow name, for example `sys/Workspace@1`.
    workflow: String,
    /// State key as a plain string. The CLI encodes it as a CBOR text key.
    key: Option<String>,
    /// In JSON mode, add a decoded `state_expanded` field alongside the raw payload.
    #[arg(long)]
    expand: bool,
    /// State key as JSON, encoded to CBOR before sending.
    #[arg(long)]
    key_json: Option<String>,
    /// State key as raw bytes encoded in base64. The CLI wraps them as a CBOR byte-string key.
    #[arg(long)]
    key_bytes_b64: Option<String>,
    /// State key as exact CBOR bytes encoded in base64.
    #[arg(long, visible_alias = "key-b64")]
    key_cbor_b64: Option<String>,
}

#[derive(Args, Debug)]
struct WorldStateCellsArgs {
    /// Workflow name, for example `sys/Workspace@1`.
    workflow: String,
}

#[derive(Args, Debug)]
#[command(about = "Read journal metadata and journal entries")]
struct WorldJournalArgs {
    #[command(subcommand)]
    cmd: WorldJournalCommand,
}

#[derive(Subcommand, Debug)]
enum WorldJournalCommand {
    /// Show the current journal head.
    Head,
    /// List journal entries from a starting height.
    Tail(WorldJournalTailArgs),
}

#[derive(Args, Debug)]
struct WorldJournalTailArgs {
    /// Starting journal height.
    #[arg(long, default_value_t = 0)]
    from: u64,
    /// Maximum number of entries to return.
    #[arg(long)]
    limit: Option<u64>,
    /// Filter by journal kind. Repeat to include multiple kinds.
    #[arg(long)]
    kind: Vec<String>,
}

#[derive(Args, Debug)]
struct WorldSendArgs {
    /// Event schema name.
    #[arg(long)]
    schema: String,
    /// JSON event value provided inline.
    #[arg(long)]
    value_json: Option<String>,
    /// File containing the JSON event value.
    #[arg(long)]
    value_file: Option<PathBuf>,
    /// Raw CBOR event value encoded as base64.
    #[arg(long)]
    value_b64: Option<String>,
    /// Optional event key encoded as base64 CBOR bytes.
    #[arg(long)]
    key_b64: Option<String>,
    /// After submission, follow the correlated trace until it reaches a terminal state.
    #[arg(long)]
    follow: bool,
    /// Correlation path used when following the trace, for example `task_id`.
    #[arg(long)]
    correlate_by: Option<String>,
    /// Correlation value used when following the trace.
    #[arg(long)]
    correlate_value: Option<String>,
    /// Poll interval for follow mode.
    #[arg(long, default_value_t = 700)]
    interval_ms: u64,
    /// Timeout for follow mode.
    #[arg(long, default_value_t = 60_000)]
    timeout_ms: u64,
    /// Fetch one or more workflow states after follow completes.
    #[arg(long)]
    result_workflow: Vec<String>,
    /// Shared string key used for all `--result-workflow` lookups.
    #[arg(long)]
    result_key: Option<String>,
    /// Shared JSON key used for all `--result-workflow` lookups.
    #[arg(long)]
    result_key_json: Option<String>,
    /// Shared raw-bytes key used for all `--result-workflow` lookups.
    #[arg(long)]
    result_key_bytes_b64: Option<String>,
    /// Shared exact CBOR key used for all `--result-workflow` lookups.
    #[arg(long)]
    result_key_cbor_b64: Option<String>,
    /// Decode `state_b64` into `state_expanded` for fetched result workflows.
    #[arg(long)]
    result_expand: bool,
    /// Workflow to read a blob ref from after result-state fetch.
    #[arg(long)]
    blob_ref_workflow: Option<String>,
    /// Dot-path inside the selected result state's expanded JSON, for example `output_ref`.
    #[arg(long)]
    blob_ref_field: Option<String>,
    /// Dot-path inside the fetched JSON blob envelope to extract, for example `assistant_text`.
    #[arg(long)]
    blob_json_field: Option<String>,
}

#[derive(Args, Debug)]
#[command(about = "Inspect queued and completed command records")]
struct WorldCmdArgs {
    #[command(subcommand)]
    cmd: WorldCmdCommand,
}

#[derive(Subcommand, Debug)]
enum WorldCmdCommand {
    /// Fetch one command by ID.
    Get(WorldCmdGetArgs),
    /// Poll one command until it reaches a terminal state.
    Wait(WorldCmdWaitArgs),
}

#[derive(Args, Debug)]
struct WorldCmdGetArgs {
    /// Command identifier.
    id: String,
}

#[derive(Args, Debug)]
struct WorldCmdWaitArgs {
    /// Command identifier.
    id: String,
    /// Poll interval in milliseconds.
    #[arg(long, default_value_t = 700)]
    interval_ms: u64,
    /// Timeout in milliseconds.
    #[arg(long, default_value_t = 60_000)]
    timeout_ms: u64,
}

#[derive(Args, Debug)]
#[command(about = "Submit governance commands for the selected world")]
struct WorldGovArgs {
    #[command(subcommand)]
    cmd: WorldGovCommand,
}

#[derive(Subcommand, Debug)]
enum WorldGovCommand {
    /// Submit a governance proposal.
    Propose(WorldGovProposeArgs),
    /// Run the governance shadow phase for a proposal.
    Shadow(WorldGovProposalIdArgs),
    /// Submit a governance approval decision.
    Approve(WorldGovApproveArgs),
    /// Apply an approved proposal.
    Apply(WorldGovProposalIdArgs),
}

#[derive(Args, Debug)]
struct WorldGovProposalIdArgs {
    /// Governance proposal identifier.
    proposal_id: u64,
    /// Actor string recorded on the governance command.
    #[arg(long)]
    actor: Option<String>,
}

#[derive(Args, Debug)]
struct WorldGovApproveArgs {
    /// Governance proposal identifier.
    proposal_id: u64,
    /// Decision string, usually `approve`.
    #[arg(long, default_value = "approve")]
    decision: String,
    /// Approver identifier recorded in governance state.
    #[arg(long, default_value = "aos")]
    approver: String,
    /// Actor string recorded on the governance command.
    #[arg(long)]
    actor: Option<String>,
    /// Human-readable approval reason.
    #[arg(long)]
    reason: Option<String>,
}

#[derive(Args, Debug)]
struct WorldGovProposeArgs {
    /// Patch file in JSON or CBOR form.
    #[arg(long)]
    patch_file: PathBuf,
    /// Actor string recorded on the governance command.
    #[arg(long)]
    actor: Option<String>,
    /// Human-readable proposal description.
    #[arg(long)]
    description: Option<String>,
}

#[derive(Args, Debug)]
#[command(about = "Inspect world traces and trace summaries")]
struct WorldTraceArgs {
    #[command(subcommand)]
    cmd: WorldTraceCommand,
}

#[derive(Subcommand, Debug)]
enum WorldTraceCommand {
    /// Fetch one trace or follow it until terminal.
    Get(WorldTraceGetArgs),
    /// Fetch a trace summary for the current world.
    Summary(WorldTraceSummaryArgs),
}

#[derive(Args, Debug)]
struct WorldTraceGetArgs {
    /// Trace root event hash.
    #[arg(long)]
    event_hash: Option<String>,
    /// Root event schema when using correlation mode.
    #[arg(long)]
    schema: Option<String>,
    /// Correlation field path when using correlation mode.
    #[arg(long)]
    correlate_by: Option<String>,
    /// Correlation value when using correlation mode.
    #[arg(long)]
    value: Option<String>,
    /// Maximum number of windows to return.
    #[arg(long)]
    window_limit: Option<u64>,
    /// Poll until the trace reaches a terminal state.
    #[arg(long)]
    follow: bool,
    /// Poll interval in milliseconds when using `--follow`.
    #[arg(long, default_value_t = 700)]
    interval_ms: u64,
}

#[derive(Args, Debug)]
struct WorldTraceSummaryArgs {
    /// Maximum number of recent items to include.
    #[arg(long)]
    recent_limit: Option<u32>,
}

#[derive(Args, Debug)]
#[command(
    long_about = "Compare the local authored bundle against the selected hosted world's current manifest, build a governance patch document, submit governance propose, and optionally chain shadow, approve, and apply. Use this for manifest-level changes such as defs, modules, routing, policies, effects, and secrets."
)]
struct WorldPatchArgs {
    /// Local world root to build and diff against the hosted manifest.
    #[arg(long)]
    local_root: Option<PathBuf>,
    /// Force a fresh workflow build instead of reusing cache.
    #[arg(long)]
    force_build: bool,
    /// Sync configured secrets from `aos.sync.json` before patch. On local targets this is a
    /// compatibility no-op; local secrets resolve from env/`.env` at world load.
    #[arg(long)]
    sync_secrets: bool,
    /// Actor string recorded on governance commands.
    #[arg(long)]
    actor: Option<String>,
    /// Human-readable proposal description.
    #[arg(long)]
    description: Option<String>,
    /// Run the shadow phase after propose.
    #[arg(long)]
    shadow: bool,
    /// Run the approve phase after shadow.
    #[arg(long)]
    approve: bool,
    /// Run the apply phase after approval.
    #[arg(long)]
    apply: bool,
    /// Wait for proposal completion even when not chaining later phases.
    #[arg(long)]
    wait: bool,
}

pub(crate) async fn handle(global: &GlobalOpts, output: OutputOpts, args: WorldArgs) -> Result<()> {
    let target = resolve_target(global)?;
    let client = ApiClient::new(&target)?;
    match args.cmd {
        WorldCommand::Ls => {
            let data = client.get_json("/v1/worlds", &[]).await?;
            print_success(output, data, None, vec![])
        }
        WorldCommand::Get(args) => {
            let world = resolve_world_arg_or_selected(&target, args.selector.as_deref())?;
            let data = client.get_json(&format!("/v1/worlds/{world}"), &[]).await?;
            print_success(output, data, None, vec![])
        }
        WorldCommand::Create(args) => handle_create(global, output, &client, &target, args).await,
        WorldCommand::Status(args) => {
            let world = resolve_world_arg_or_selected(&target, args.selector.as_deref())?;
            let data = client
                .get_json(&format!("/v1/worlds/{world}/runtime"), &[])
                .await?;
            print_success(output, data, None, vec![])
        }
        WorldCommand::Manifest(args) => {
            let world = resolve_world_arg_or_selected(&target, args.selector.as_deref())?;
            let data = client
                .get_json(&format!("/v1/worlds/{world}/manifest"), &[])
                .await?;
            print_success(output, data, None, vec![])
        }
        WorldCommand::Defs(args) => handle_defs(output, &client, &target, args).await,
        WorldCommand::State(args) => handle_state(output, &client, &target, args).await,
        WorldCommand::Journal(args) => handle_journal(output, &client, &target, args).await,
        WorldCommand::Send(args) => handle_send(output, &client, &target, args).await,
        WorldCommand::Cmd(args) => handle_cmd(output, &client, &target, args).await,
        WorldCommand::Gov(args) => handle_gov(output, &client, &target, args).await,
        WorldCommand::Trace(args) => handle_trace(output, &client, &target, args).await,
        WorldCommand::Patch(args) => handle_patch(output, &client, &target, args).await,
    }
}

async fn handle_send(
    output: OutputOpts,
    client: &ApiClient,
    target: &crate::client::ApiTarget,
    args: WorldSendArgs,
) -> Result<()> {
    let world = resolve_world_arg_or_selected(target, None)?;
    let value = event_value(&args)?;
    let event = client
        .post_json(
            &format!("/v1/worlds/{world}/events"),
            &json!({
                "schema": args.schema,
                "value_json": value,
                "value_b64": args.value_b64,
                "key_b64": args.key_b64,
            }),
        )
        .await?;

    let wants_follow = args.follow
        || !args.result_workflow.is_empty()
        || args.blob_ref_workflow.is_some()
        || args.blob_ref_field.is_some()
        || args.blob_json_field.is_some();
    if !wants_follow {
        return print_success(output, event, None, vec![]);
    }

    if args.correlate_by.is_none() || args.correlate_value.is_none() {
        return Err(anyhow!(
            "world send follow/result mode requires both --correlate-by and --correlate-value"
        ));
    }

    let trace = wait_for_trace_terminal(
        client,
        &world,
        &args.schema,
        args.correlate_by.as_deref().unwrap(),
        args.correlate_value.as_deref().unwrap(),
        args.interval_ms,
        args.timeout_ms,
    )
    .await?;

    let state_key_b64 = encode_send_result_key_query(&args)?;
    if (!args.result_workflow.is_empty() || args.blob_ref_workflow.is_some())
        && state_key_b64.is_none()
    {
        return Err(anyhow!(
            "result-state lookup requires one of --result-key, --result-key-json, --result-key-bytes-b64, or --result-key-cbor-b64"
        ));
    }

    let mut states = serde_json::Map::new();
    for workflow in &args.result_workflow {
        let data = fetch_result_state(
            client,
            &world,
            workflow,
            state_key_b64.as_deref(),
            args.result_expand,
        )
        .await?;
        states.insert(workflow.clone(), data);
    }

    let mut blob_ref = None;
    if let Some(workflow) = &args.blob_ref_workflow {
        let state = if let Some(existing) = states.get(workflow) {
            existing.clone()
        } else {
            fetch_result_state(client, &world, workflow, state_key_b64.as_deref(), true).await?
        };
        let field = args
            .blob_ref_field
            .as_deref()
            .ok_or_else(|| anyhow!("--blob-ref-workflow requires --blob-ref-field"))?;
        let path = normalize_json_field_path(field);
        blob_ref = extract_state_field(&state, &path)
            .and_then(Value::as_str)
            .map(ToString::to_string);
        states.entry(workflow.clone()).or_insert(state);
    } else if args.blob_ref_field.is_some() || args.blob_json_field.is_some() {
        return Err(anyhow!(
            "--blob-ref-field/--blob-json-field require --blob-ref-workflow"
        ));
    }

    let blob_json = if let Some(blob_ref) = &blob_ref {
        Some(fetch_blob_json(client, &world, blob_ref).await?)
    } else {
        None
    };
    let blob_value = if let (Some(blob_json), Some(field)) = (&blob_json, &args.blob_json_field) {
        extract_json_path(blob_json, &normalize_json_field_path(field)).cloned()
    } else {
        None
    };

    let mut data = serde_json::Map::new();
    data.insert("event".into(), event);
    data.insert("trace".into(), trace);
    if !states.is_empty() {
        data.insert("states".into(), Value::Object(states));
    }
    if let Some(blob_ref) = blob_ref {
        data.insert("blob_ref".into(), Value::String(blob_ref));
    }
    if let Some(blob_json) = blob_json {
        data.insert("blob_json".into(), blob_json);
    }
    if let Some(blob_value) = blob_value {
        data.insert("blob_value".into(), blob_value);
    }
    print_success(output, Value::Object(data), None, vec![])
}

async fn handle_create(
    global: &GlobalOpts,
    output: OutputOpts,
    client: &ApiClient,
    target: &crate::client::ApiTarget,
    mut args: WorldCreateArgs,
) -> Result<()> {
    if should_default_local_root(target, &args)? {
        args.local_root = Some(std::env::current_dir().context("get current directory")?);
    }

    let secret_universe = if target.kind == crate::config::ProfileKind::Local {
        None
    } else if let Some(universe_id) = args.universe_id.clone() {
        Some(universe_id)
    } else {
        None
    };
    let using_local_root = args.local_root.is_some();
    let using_manifest_hash = args.manifest_hash.is_some();
    let using_from_world = args.from_world.is_some();
    let source_count = using_local_root as u8 + using_manifest_hash as u8 + using_from_world as u8;
    if source_count == 0 {
        return Err(anyhow!(
            "world create requires one of --local-root, --manifest-hash, or --from-world"
        ));
    }
    if source_count > 1 {
        return Err(anyhow!(
            "world create accepts exactly one source: --local-root, --manifest-hash, or --from-world"
        ));
    }
    if args.upload_only && !using_local_root {
        return Err(anyhow!(
            "world create --upload-only requires --local-root and cannot be combined with --manifest-hash or --from-world"
        ));
    }
    if args.sync_secrets && !using_local_root {
        return Err(anyhow!("world create --sync-secrets requires --local-root"));
    }

    if args.local_root.is_some() {
        let is_local_target = target.kind == crate::config::ProfileKind::Local;
        let dirs = resolve_local_dirs(args.local_root.as_deref())?;
        if args.sync_secrets {
            if is_local_target {
                print_verbose(
                    output,
                    "local --sync-secrets is a compatibility no-op; local secrets resolve from env/.env at world load",
                );
            } else {
                print_verbose(output, "syncing hosted secrets from aos.sync.json");
                let synced = sync_hosted_secrets(
                    client,
                    secret_universe.as_deref(),
                    args.local_root.as_deref(),
                    None,
                    None,
                )
                .await?;
                print_verbose(
                    output,
                    format!(
                        "synced {} secrets, {} unchanged",
                        synced
                            .get("synced")
                            .and_then(Value::as_array)
                            .map(|values| values.len())
                            .unwrap_or(0),
                        synced
                            .get("unchanged")
                            .and_then(Value::as_array)
                            .map(|values| values.len())
                            .unwrap_or(0)
                    ),
                );
            }
        }
        print_verbose(
            output,
            format!(
                "building local AIR/workflow bundle from root {} (air {}, workflow {})",
                dirs.root.display(),
                dirs.air_dir.display(),
                dirs.workflow_dir.display()
            ),
        );
        let (store, bundle, mut warnings) =
            build_bundle_from_world(args.local_root.as_deref(), args.force_build)?;
        if !is_local_target {
            warnings.extend(
                secret_binding_readiness_warnings(
                    client,
                    secret_universe.as_deref(),
                    &bundle.manifest,
                    &bundle.secrets,
                )
                .await,
            );
        }
        print_verbose(output, "uploading bundle to node CAS");
        let uploaded = upload_bundle(client, &store, &bundle, warnings, Some(&dirs)).await?;
        if args.upload_only {
            return print_success(
                output,
                json!({ "manifest_hash": uploaded.manifest_hash }),
                None,
                uploaded.warnings,
            );
        }
        print_verbose(
            output,
            format!("creating world from manifest {}", uploaded.manifest_hash),
        );
        let mut body = serde_json::Map::new();
        body.insert("world_id".into(), serde_json::to_value(args.world_id)?);
        if let Some(universe_id) = args.universe_id.as_ref() {
            body.insert("universe_id".into(), Value::String(universe_id.clone()));
        }
        body.insert(
            "source".into(),
            json!({
                "kind": "manifest",
                "manifest_hash": uploaded.manifest_hash,
            }),
        );
        let data = client.post_json("/v1/worlds", &Value::Object(body)).await?;
        if args.select {
            let world_id = created_world_id(&data)?;
            select_created_world(global, &world_id)?;
        }
        return print_success(output, data, None, uploaded.warnings);
    }

    if let Some(source_selector) = args.from_world.as_deref() {
        let snapshot = if let Some(height) = args.snapshot_height {
            json!({ "kind": "by_height", "height": height })
        } else if let Some(snapshot_ref) = args.snapshot_ref {
            json!({ "kind": "by_ref", "snapshot_ref": snapshot_ref })
        } else {
            json!({ "kind": "active_baseline" })
        };
        let source_world = resolve_world_selector(source_selector)?;
        let data = client
            .post_json(
                &format!("/v1/worlds/{source_world}/fork"),
                &json!({
                    "src_snapshot": snapshot,
                    "new_world_id": args.world_id,
                }),
            )
            .await?;
        if args.select {
            let world_id = created_world_id(&data)?;
            select_created_world(global, &world_id)?;
        }
        return print_success(output, data, None, vec![]);
    }

    let manifest_hash = args.manifest_hash.ok_or_else(|| {
        anyhow!("world create requires --manifest-hash when not using --local-root or --from-world")
    })?;
    let mut body = serde_json::Map::new();
    body.insert("world_id".into(), serde_json::to_value(args.world_id)?);
    if let Some(universe_id) = args.universe_id.as_ref() {
        body.insert("universe_id".into(), Value::String(universe_id.clone()));
    }
    body.insert(
        "source".into(),
        json!({
            "kind": "manifest",
            "manifest_hash": manifest_hash,
        }),
    );
    let data = client.post_json("/v1/worlds", &Value::Object(body)).await?;
    if args.select {
        let world_id = created_world_id(&data)?;
        select_created_world(global, &world_id)?;
    }
    print_success(output, data, None, vec![])
}

fn created_world_id(data: &Value) -> Result<String> {
    data.get("world_id")
        .or_else(|| data.get("record").and_then(|value| value.get("world_id")))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("world create response did not include world_id"))
}

fn select_created_world(global: &GlobalOpts, world_id: &str) -> Result<()> {
    let paths = ConfigPaths::resolve(global.config.as_deref())?;
    let mut config = load_config(&paths)?;
    let profile_name = global
        .profile
        .clone()
        .or_else(|| config.current_profile.clone())
        .ok_or_else(|| anyhow!("cannot --select created world without a saved current profile"))?;
    let profile = config
        .profiles
        .get_mut(&profile_name)
        .ok_or_else(|| anyhow!("profile '{profile_name}' not found"))?;
    profile.world = Some(world_id.to_string());
    config.current_profile = Some(profile_name);
    save_config(&paths, &config)
}

fn should_default_local_root(
    target: &crate::client::ApiTarget,
    args: &WorldCreateArgs,
) -> Result<bool> {
    if target.kind != crate::config::ProfileKind::Local {
        return Ok(false);
    }
    if args.local_root.is_some() || args.manifest_hash.is_some() || args.from_world.is_some() {
        return Ok(false);
    }
    let dirs = resolve_local_dirs(None)?;
    Ok(dirs.air_dir.exists())
}

async fn handle_defs(
    output: OutputOpts,
    client: &ApiClient,
    target: &crate::client::ApiTarget,
    args: WorldDefsArgs,
) -> Result<()> {
    let world = resolve_world_arg_or_selected(target, None)?;
    match args.cmd {
        WorldDefsCommand::Ls(args) => {
            let data = client
                .get_json(
                    &format!("/v1/worlds/{world}/defs"),
                    &[("kinds", args.kinds), ("prefix", args.prefix)],
                )
                .await?;
            print_success(output, data, None, vec![])
        }
        WorldDefsCommand::Get(args) => {
            let kind = encode_path_segment(&args.kind);
            let name = encode_path_segment(&args.name);
            let data = client
                .get_json(&format!("/v1/worlds/{world}/defs/{kind}/{name}"), &[])
                .await?;
            print_success(output, data, None, vec![])
        }
    }
}

async fn handle_state(
    output: OutputOpts,
    client: &ApiClient,
    target: &crate::client::ApiTarget,
    args: WorldStateArgs,
) -> Result<()> {
    let world = resolve_world_arg_or_selected(target, None)?;
    match args.cmd {
        WorldStateCommand::Ls => {
            let manifest: ManifestEnvelope = serde_json::from_value(
                client
                    .get_json(&format!("/v1/worlds/{world}/manifest"), &[])
                    .await?,
            )
            .context("decode manifest response")?;
            let mut workflows = Vec::new();
            for module in manifest.manifest.modules {
                let kind = encode_path_segment("module");
                let name = encode_path_segment(module.name.as_str());
                let def: DefEnvelope = serde_json::from_value(
                    client
                        .get_json(&format!("/v1/worlds/{world}/defs/{kind}/{name}"), &[])
                        .await?,
                )
                .with_context(|| format!("decode module def {}", module.name.as_str()))?;
                if matches!(
                    def.def,
                    AirNode::Defmodule(ref module_def)
                        if matches!(module_def.module_kind, ModuleKind::Workflow)
                ) {
                    workflows.push(module.name.to_string());
                }
            }
            workflows.sort();
            print_success(output, json!(workflows), None, vec![])
        }
        WorldStateCommand::Get(args) => {
            let workflow = encode_path_segment(&args.workflow);
            let key_b64 = encode_state_key_query(&args)?;
            let data = client
                .get_json(
                    &format!("/v1/worlds/{world}/state/{workflow}"),
                    &[("key_b64", key_b64)],
                )
                .await?;
            print_state_get(output, data, args.expand)
        }
        WorldStateCommand::Cells(args) => {
            let workflow = encode_path_segment(&args.workflow);
            let data = client
                .get_json(&format!("/v1/worlds/{world}/state/{workflow}/cells"), &[])
                .await?;
            print_state_cells(output, data)
        }
    }
}

async fn handle_journal(
    output: OutputOpts,
    client: &ApiClient,
    target: &crate::client::ApiTarget,
    args: WorldJournalArgs,
) -> Result<()> {
    let world = resolve_world_arg_or_selected(target, None)?;
    match args.cmd {
        WorldJournalCommand::Head => {
            let data = client
                .get_json(&format!("/v1/worlds/{world}/journal/head"), &[])
                .await?;
            print_success(output, data, None, vec![])
        }
        WorldJournalCommand::Tail(args) => {
            let data = client
                .get_json(
                    &format!("/v1/worlds/{world}/journal"),
                    &[
                        ("from", Some(args.from.to_string())),
                        ("limit", args.limit.map(|v| v.to_string())),
                        (
                            "kinds",
                            if args.kind.is_empty() {
                                None
                            } else {
                                Some(args.kind.join(","))
                            },
                        ),
                    ],
                )
                .await?;
            print_journal_entries(output, data)
        }
    }
}

async fn handle_cmd(
    output: OutputOpts,
    client: &ApiClient,
    target: &crate::client::ApiTarget,
    args: WorldCmdArgs,
) -> Result<()> {
    let world = resolve_world_arg_or_selected(target, None)?;
    match args.cmd {
        WorldCmdCommand::Get(args) => {
            let data = fetch_command(client, &world, &args.id).await?;
            print_success(output, data, None, vec![])
        }
        WorldCmdCommand::Wait(args) => {
            let data =
                wait_for_command(client, &world, &args.id, args.interval_ms, args.timeout_ms)
                    .await?;
            print_success(output, data, None, vec![])
        }
    }
}

async fn handle_gov(
    output: OutputOpts,
    client: &ApiClient,
    target: &crate::client::ApiTarget,
    args: WorldGovArgs,
) -> Result<()> {
    let world = resolve_world_arg_or_selected(target, None)?;
    match args.cmd {
        WorldGovCommand::Propose(args) => {
            let bytes = fs::read(&args.patch_file)
                .with_context(|| format!("read {}", args.patch_file.display()))?;
            let params = if args.patch_file.extension().and_then(|ext| ext.to_str()) == Some("json")
            {
                let patch: Value = serde_json::from_slice(&bytes).context("parse patch json")?;
                let patch_hash = upload_patch_json(client, &world, &patch).await?;
                GovProposeParams {
                    patch: GovPatchInput::PatchBlobRef {
                        blob_ref: HashRef::new(patch_hash).context("build patch hash ref")?,
                        format: "patch_doc_json".into(),
                    },
                    summary: None,
                    manifest_base: None,
                    description: args.description,
                }
            } else {
                let patch_hash = upload_patch_bytes(client, &world, &bytes).await?;
                GovProposeParams {
                    patch: GovPatchInput::PatchBlobRef {
                        blob_ref: HashRef::new(patch_hash).context("build patch hash ref")?,
                        format: "manifest_patch_cbor".into(),
                    },
                    summary: None,
                    manifest_base: None,
                    description: args.description,
                }
            };
            let body = serde_json::to_value(&params).context("encode governance propose body")?;
            let data = client
                .post_json(
                    &format!("/v1/worlds/{world}/governance/propose"),
                    &merge_actor(args.actor, body),
                )
                .await?;
            print_success(output, data, None, vec![])
        }
        WorldGovCommand::Shadow(args) => {
            let data = client
                .post_json(
                    &format!("/v1/worlds/{world}/governance/shadow"),
                    &json!({ "proposal_id": args.proposal_id, "actor": args.actor }),
                )
                .await?;
            print_success(output, data, None, vec![])
        }
        WorldGovCommand::Approve(args) => {
            let decision = parse_gov_decision(&args.decision)?;
            let body = serde_json::to_value(&GovApproveParams {
                proposal_id: args.proposal_id,
                decision,
                approver: args.approver,
                reason: args.reason,
            })
            .context("encode governance approve body")?;
            let data = client
                .post_json(
                    &format!("/v1/worlds/{world}/governance/approve"),
                    &merge_actor(args.actor, body),
                )
                .await?;
            print_success(output, data, None, vec![])
        }
        WorldGovCommand::Apply(args) => {
            let data = client
                .post_json(
                    &format!("/v1/worlds/{world}/governance/apply"),
                    &json!({ "proposal_id": args.proposal_id, "actor": args.actor }),
                )
                .await?;
            print_success(output, data, None, vec![])
        }
    }
}

async fn handle_trace(
    output: OutputOpts,
    client: &ApiClient,
    target: &crate::client::ApiTarget,
    args: WorldTraceArgs,
) -> Result<()> {
    let world = resolve_world_arg_or_selected(target, None)?;
    match args.cmd {
        WorldTraceCommand::Get(args) => {
            let query = vec![
                ("event_hash", args.event_hash.clone()),
                ("schema", args.schema.clone()),
                ("correlate_by", args.correlate_by.clone()),
                ("value", args.value.clone()),
                ("window_limit", args.window_limit.map(|v| v.to_string())),
            ];
            if args.follow {
                loop {
                    let data = client
                        .get_json(&format!("/v1/worlds/{world}/trace"), &query)
                        .await?;
                    let terminal = data
                        .get("terminal_state")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    if is_terminal_state(terminal) {
                        return print_trace(output, data);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(args.interval_ms)).await;
                }
            }
            let data = client
                .get_json(&format!("/v1/worlds/{world}/trace"), &query)
                .await?;
            print_trace(output, data)
        }
        WorldTraceCommand::Summary(args) => {
            let data = client
                .get_json(
                    &format!("/v1/worlds/{world}/trace-summary"),
                    &[("recent_limit", args.recent_limit.map(|v| v.to_string()))],
                )
                .await?;
            print_trace_summary(output, data)
        }
    }
}

async fn handle_patch(
    output: OutputOpts,
    client: &ApiClient,
    target: &crate::client::ApiTarget,
    args: WorldPatchArgs,
) -> Result<()> {
    let world = resolve_world_arg_or_selected(target, None)?;
    let is_local_target = target.kind == crate::config::ProfileKind::Local;
    if args.sync_secrets {
        if is_local_target {
            print_verbose(
                output,
                "local --sync-secrets is a compatibility no-op; local secrets resolve from env/.env at world load",
            );
        } else {
            print_verbose(output, "syncing hosted secrets from aos.sync.json");
            let universe_id = super::common::universe_id_for_world(client, &world).await?;
            let synced = sync_hosted_secrets(
                client,
                Some(&universe_id),
                args.local_root.as_deref(),
                None,
                args.actor.as_deref(),
            )
            .await?;
            print_verbose(
                output,
                format!(
                    "synced {} secrets, {} unchanged",
                    synced
                        .get("synced")
                        .and_then(Value::as_array)
                        .map(|values| values.len())
                        .unwrap_or(0),
                    synced
                        .get("unchanged")
                        .and_then(Value::as_array)
                        .map(|values| values.len())
                        .unwrap_or(0)
                ),
            );
        }
    }
    let dirs = resolve_local_dirs(args.local_root.as_deref())?;
    print_verbose(
        output,
        format!(
            "building local AIR/workflow bundle from root {} (air {}, workflow {})",
            dirs.root.display(),
            dirs.air_dir.display(),
            dirs.workflow_dir.display()
        ),
    );
    let (store, bundle, mut warnings) =
        build_bundle_from_world(args.local_root.as_deref(), args.force_build)?;
    if !is_local_target {
        let universe_id = super::common::universe_id_for_world(client, &world).await?;
        warnings.extend(
            secret_binding_readiness_warnings(
                client,
                Some(&universe_id),
                &bundle.manifest,
                &bundle.secrets,
            )
            .await,
        );
    }
    print_verbose(
        output,
        format!("fetching current manifest for world {world}"),
    );
    let remote = fetch_remote_manifest(client, &world).await?;
    let local_manifest_hash = Hash::of_bytes(&to_canonical_cbor(&bundle.manifest)?).to_hex();
    if local_manifest_hash == remote.manifest_hash {
        return print_success(
            output,
            json!({
                "status": "noop",
                "manifest_hash": local_manifest_hash,
                "world": world,
            }),
            None,
            warnings,
        );
    }
    print_verbose(output, "uploading bundle to hosted CAS");
    let uploaded = upload_bundle(client, &store, &bundle, warnings, Some(&dirs)).await?;
    print_verbose(output, "building governance patch document");
    let patch = build_patch(&remote, &bundle)?;
    let patch_hash = upload_patch_json(client, &world, &patch).await?;
    print_verbose(output, "submitting governance propose command");
    let propose_body = serde_json::to_value(&GovProposeParams {
        patch: GovPatchInput::PatchBlobRef {
            blob_ref: HashRef::new(patch_hash).context("build patch hash ref")?,
            format: "patch_doc_json".into(),
        },
        summary: None,
        manifest_base: None,
        description: args.description.clone(),
    })
    .context("encode governance propose body")?;
    let propose = client
        .post_json(
            &format!("/v1/worlds/{world}/governance/propose"),
            &merge_actor(args.actor.clone(), propose_body),
        )
        .await?;
    let mut result = json!({
        "manifest_hash": uploaded.manifest_hash,
        "propose": propose.clone(),
    });
    let should_chain = args.shadow || args.approve || args.apply;
    if should_chain || args.wait {
        let propose_command_id = propose
            .get("command_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("governance propose response missing command_id"))?;
        let propose_record =
            wait_for_command(client, &world, propose_command_id, 700, 60_000).await?;
        result["propose_result"] = propose_record.clone();

        if should_chain {
            let propose_receipt: GovProposeReceipt =
                decode_command_payload(client, &world, &propose_record).await?;
            let proposal_id = propose_receipt.proposal_id;
            result["proposal_id"] = json!(proposal_id);

            if args.shadow || args.approve || args.apply {
                let shadow = client
                    .post_json(
                        &format!("/v1/worlds/{world}/governance/shadow"),
                        &json!({
                            "proposal_id": proposal_id,
                            "actor": args.actor,
                        }),
                    )
                    .await?;
                result["shadow"] = shadow.clone();
                let shadow_command_id = shadow
                    .get("command_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("governance shadow response missing command_id"))?;
                let shadow_record =
                    wait_for_command(client, &world, shadow_command_id, 700, 60_000).await?;
                result["shadow_result"] = shadow_record.clone();
                let _: GovShadowReceipt =
                    decode_command_payload(client, &world, &shadow_record).await?;
            }

            if args.approve || args.apply {
                let approve_body = serde_json::to_value(&GovApproveParams {
                    proposal_id,
                    decision: GovDecision::Approve,
                    approver: default_approver(),
                    reason: None,
                })
                .context("encode governance approve body")?;
                let approve = client
                    .post_json(
                        &format!("/v1/worlds/{world}/governance/approve"),
                        &merge_actor(args.actor.clone(), approve_body),
                    )
                    .await?;
                result["approve"] = approve.clone();
                let approve_command_id = approve
                    .get("command_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("governance approve response missing command_id"))?;
                let approve_record =
                    wait_for_command(client, &world, approve_command_id, 700, 60_000).await?;
                result["approve_result"] = approve_record.clone();
                let approve_receipt: GovApproveReceipt =
                    decode_command_payload(client, &world, &approve_record).await?;
                ensure_approved(approve_receipt.decision, proposal_id)?;
            }

            if args.apply {
                let apply = client
                    .post_json(
                        &format!("/v1/worlds/{world}/governance/apply"),
                        &json!({
                            "proposal_id": proposal_id,
                            "actor": args.actor,
                        }),
                    )
                    .await?;
                result["apply"] = apply.clone();
                let apply_command_id = apply
                    .get("command_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("governance apply response missing command_id"))?;
                let apply_record =
                    wait_for_command(client, &world, apply_command_id, 700, 60_000).await?;
                result["apply_result"] = apply_record.clone();
                let _: GovApplyReceipt =
                    decode_command_payload(client, &world, &apply_record).await?;
            }
        }
    }
    print_success(output, result, None, uploaded.warnings)
}

async fn secret_binding_readiness_warnings(
    client: &ApiClient,
    universe_id: Option<&str>,
    manifest: &Manifest,
    secret_defs: &[DefSecret],
) -> Vec<String> {
    let declared = declared_secret_binding_ids(manifest, secret_defs);
    if declared.is_empty() {
        return Vec::new();
    }

    let bound = match fetch_active_secret_binding_ids(client, universe_id).await {
        Ok(bound) => bound,
        Err(err) => {
            return vec![format!(
                "could not evaluate secret binding readiness: {err}"
            )];
        }
    };

    let missing: Vec<String> = declared
        .iter()
        .filter(|binding| !bound.contains(binding.as_str()))
        .cloned()
        .collect();
    if missing.is_empty() {
        return Vec::new();
    }

    vec![
        format!(
            "world declares secret bindings [{}] but the selected universe is missing [{}]; runtime paths that require them may fail",
            declared.join(", "),
            missing.join(", "),
        ),
        "sync them with `aos world patch --sync-secrets` or bind them manually with `aos universe secret binding set ... --universe-id ...`".into(),
    ]
}

async fn fetch_active_secret_binding_ids(
    client: &ApiClient,
    universe_id: Option<&str>,
) -> Result<BTreeSet<String>> {
    let data = client
        .get_json(
            "/v1/secrets/bindings",
            &[(
                "universe_id",
                universe_id.map(std::string::ToString::to_string),
            )],
        )
        .await?;
    let records = parse_secret_binding_records(&data)?;
    Ok(records
        .into_iter()
        .filter(|record| matches!(record.status, SecretBindingStatus::Active))
        .map(|record| record.binding_id)
        .collect())
}

fn parse_secret_binding_records(data: &Value) -> Result<Vec<SecretBindingRecord>> {
    serde_json::from_value::<Vec<SecretBindingRecord>>(data.clone())
        .or_else(|_| {
            data.get("items")
                .cloned()
                .ok_or_else(|| anyhow!("secret binding list response missing items"))
                .and_then(|value| {
                    serde_json::from_value::<Vec<SecretBindingRecord>>(value)
                        .context("decode secret binding list response items")
                })
        })
        .context("decode secret binding list response")
}

fn declared_secret_binding_ids(manifest: &Manifest, secret_defs: &[DefSecret]) -> Vec<String> {
    let defs_by_name: BTreeMap<&str, &str> = secret_defs
        .iter()
        .map(|secret| (secret.name.as_str(), secret.binding_id.as_str()))
        .collect();
    let mut binding_ids = BTreeSet::new();
    for secret in &manifest.secrets {
        match secret {
            SecretEntry::Decl(secret) => {
                let binding_id = secret.binding_id.trim();
                if !binding_id.is_empty() {
                    binding_ids.insert(binding_id.to_string());
                }
            }
            SecretEntry::Ref(secret) => {
                if let Some(binding_id) = defs_by_name.get(secret.name.as_str()) {
                    let binding_id = binding_id.trim();
                    if !binding_id.is_empty() {
                        binding_ids.insert(binding_id.to_string());
                    }
                }
            }
        }
    }
    binding_ids.into_iter().collect()
}

fn merge_actor(actor: Option<String>, body: Value) -> Value {
    match body {
        Value::Object(mut map) => {
            if let Some(actor) = actor {
                map.insert("actor".into(), Value::String(actor));
            }
            Value::Object(map)
        }
        other => other,
    }
}

fn parse_gov_decision(value: &str) -> Result<GovDecision> {
    match value.trim().to_ascii_lowercase().as_str() {
        "approve" => Ok(GovDecision::Approve),
        "reject" => Ok(GovDecision::Reject),
        other => Err(anyhow!(
            "invalid governance decision '{other}' (expected approve or reject)"
        )),
    }
}

fn event_value(args: &WorldSendArgs) -> Result<Option<Value>> {
    match (&args.value_json, &args.value_file, &args.value_b64) {
        (_, _, Some(_)) => Ok(None),
        (Some(raw), None, None) => {
            let value = serde_json::from_str(raw).context("parse --value-json")?;
            Ok(Some(value))
        }
        (None, Some(path), None) => {
            let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
            let value = serde_json::from_slice(&bytes)
                .with_context(|| format!("parse {}", path.display()))?;
            Ok(Some(value))
        }
        (None, None, None) => Err(anyhow!(
            "world send requires one of --value-json, --value-file, or --value-b64"
        )),
        _ => Err(anyhow!("world send accepts exactly one value source")),
    }
}

async fn wait_for_trace_terminal(
    client: &ApiClient,
    world: &str,
    schema: &str,
    correlate_by: &str,
    correlate_value: &str,
    interval_ms: u64,
    timeout_ms: u64,
) -> Result<Value> {
    let started = Instant::now();
    let interval = Duration::from_millis(interval_ms.max(1));
    let timeout = Duration::from_millis(timeout_ms.max(1));
    let query = vec![
        ("schema", Some(schema.to_string())),
        ("correlate_by", Some(correlate_by.to_string())),
        ("value", Some(correlate_value.to_string())),
    ];
    loop {
        let data = match client
            .get_json(&format!("/v1/worlds/{world}/trace"), &query)
            .await
        {
            Ok(data) => data,
            Err(err) => {
                let message = err.to_string();
                let retryable = message.contains("trace root event for correlation query")
                    || message.contains("trace root event not found for correlation query")
                    || message.contains("not_found");
                if retryable && started.elapsed() < timeout {
                    tokio::time::sleep(interval).await;
                    continue;
                }
                return Err(err);
            }
        };
        let terminal = data
            .get("terminal_state")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if is_terminal_state(terminal) {
            return Ok(data);
        }
        if started.elapsed() >= timeout {
            return Err(anyhow!(
                "timed out waiting for terminal trace state for schema '{}' correlation '{}={}'",
                schema,
                correlate_by,
                correlate_value
            ));
        }
        tokio::time::sleep(interval).await;
    }
}

async fn fetch_result_state(
    client: &ApiClient,
    world: &str,
    workflow: &str,
    key_b64: Option<&str>,
    expand: bool,
) -> Result<Value> {
    let mut query = Vec::new();
    if let Some(key_b64) = key_b64 {
        query.push(("key_b64", Some(key_b64.to_string())));
    }
    let workflow = encode_path_segment(workflow);
    let mut data = client
        .get_json(&format!("/v1/worlds/{world}/state/{workflow}"), &query)
        .await?;
    if expand {
        data = augment_state_get_json(data)?;
    }
    Ok(data)
}

async fn fetch_blob_json(client: &ApiClient, world: &str, blob_ref: &str) -> Result<Value> {
    let bytes = client
        .get_bytes(
            &format!("/v1/cas/blobs/{blob_ref}"),
            &universe_query_for_world(client, world).await?,
        )
        .await?;
    serde_json::from_slice(&bytes).context("decode blob json")
}

fn normalize_json_field_path(value: &str) -> Vec<&str> {
    value
        .trim()
        .trim_start_matches('$')
        .trim_start_matches('.')
        .split('.')
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn extract_json_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for segment in path {
        current = match current {
            Value::Object(map) => map.get(*segment)?,
            _ => return None,
        };
    }
    Some(current)
}

fn extract_state_field<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    if let Some(found) = extract_json_path(value, path) {
        return Some(found);
    }
    if path.first().copied() == Some("state_expanded") {
        return None;
    }
    let Some(state_expanded) = value.get("state_expanded") else {
        return None;
    };
    extract_json_path(state_expanded, path)
}

fn augment_state_get_json(mut data: Value) -> Result<Value> {
    let Some(map) = data.as_object_mut() else {
        return Ok(data);
    };
    let Some(state_b64) = map.get("state_b64").and_then(Value::as_str) else {
        return Ok(data);
    };
    let bytes = BASE64_STANDARD
        .decode(state_b64)
        .with_context(|| format!("decode state payload '{state_b64}'"))?;
    let expanded = crate::render::decode_payload_display_value(&bytes);
    map.insert("state_expanded".into(), expanded);
    Ok(data)
}

fn encode_send_result_key_query(args: &WorldSendArgs) -> Result<Option<String>> {
    let mut sources = 0u8;
    if args.result_key.is_some() {
        sources += 1;
    }
    if args.result_key_json.is_some() {
        sources += 1;
    }
    if args.result_key_bytes_b64.is_some() {
        sources += 1;
    }
    if args.result_key_cbor_b64.is_some() {
        sources += 1;
    }
    if sources > 1 {
        return Err(anyhow!(
            "send result lookup accepts only one key source: --result-key, --result-key-json, --result-key-bytes-b64, or --result-key-cbor-b64"
        ));
    }
    if let Some(key) = &args.result_key {
        return Ok(Some(BASE64_STANDARD.encode(
            to_canonical_cbor(key).context("encode result string key as canonical cbor")?,
        )));
    }
    if let Some(key_json) = &args.result_key_json {
        let value: Value = serde_json::from_str(key_json).context("parse --result-key-json")?;
        return Ok(Some(BASE64_STANDARD.encode(
            to_canonical_cbor(&value).context("encode result json key as canonical cbor")?,
        )));
    }
    if let Some(key_bytes_b64) = &args.result_key_bytes_b64 {
        let bytes = BASE64_STANDARD
            .decode(key_bytes_b64)
            .context("decode --result-key-bytes-b64")?;
        return Ok(Some(
            BASE64_STANDARD.encode(
                to_canonical_cbor(&serde_bytes::ByteBuf::from(bytes))
                    .context("encode result byte-string key as canonical cbor")?,
            ),
        ));
    }
    if let Some(key_cbor_b64) = &args.result_key_cbor_b64 {
        let bytes = BASE64_STANDARD
            .decode(key_cbor_b64)
            .context("decode --result-key-cbor-b64")?;
        return Ok(Some(BASE64_STANDARD.encode(bytes)));
    }
    Ok(None)
}

fn encode_state_key_query(args: &WorldStateGetArgs) -> Result<Option<String>> {
    let mut sources = 0u8;
    if args.key.is_some() {
        sources += 1;
    }
    if args.key_json.is_some() {
        sources += 1;
    }
    if args.key_bytes_b64.is_some() {
        sources += 1;
    }
    if args.key_cbor_b64.is_some() {
        sources += 1;
    }
    if sources > 1 {
        return Err(anyhow!(
            "state get accepts only one key source: positional <key>, --key-json, --key-bytes-b64, or --key-cbor-b64"
        ));
    }
    if let Some(key) = &args.key {
        return Ok(Some(BASE64_STANDARD.encode(
            to_canonical_cbor(key).context("encode string key as canonical cbor")?,
        )));
    }
    if let Some(key_json) = &args.key_json {
        let value: Value = serde_json::from_str(key_json).context("parse --key-json")?;
        return Ok(Some(BASE64_STANDARD.encode(
            to_canonical_cbor(&value).context("encode json key as canonical cbor")?,
        )));
    }
    if let Some(key_bytes_b64) = &args.key_bytes_b64 {
        let bytes = BASE64_STANDARD
            .decode(key_bytes_b64)
            .context("decode --key-bytes-b64")?;
        return Ok(Some(
            BASE64_STANDARD.encode(
                to_canonical_cbor(&serde_cbor::Value::Bytes(bytes))
                    .context("encode byte key as canonical cbor")?,
            ),
        ));
    }
    Ok(args.key_cbor_b64.clone())
}

#[derive(Debug, Deserialize)]
struct ManifestEnvelope {
    manifest: Manifest,
}

#[derive(Debug, Deserialize)]
struct DefEnvelope {
    def: AirNode,
}

#[cfg(test)]
mod tests {
    use super::{
        WorldCreateArgs, WorldStateGetArgs, created_world_id, declared_secret_binding_ids,
        encode_state_key_query, parse_secret_binding_records, should_default_local_root,
    };
    use aos_air_types::{DefSecret, HashRef, Manifest, NamedRef, SecretEntry};
    use aos_node::{SecretBindingSourceKind, SecretBindingStatus};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use serde_json::json;
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    fn cwd_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn local_target() -> crate::client::ApiTarget {
        crate::client::ApiTarget {
            kind: crate::config::ProfileKind::Local,
            api: "http://127.0.0.1:9010".into(),
            headers: Default::default(),
            token: None,
            verbose: false,
            world: None,
        }
    }

    fn remote_target() -> crate::client::ApiTarget {
        crate::client::ApiTarget {
            kind: crate::config::ProfileKind::Remote,
            api: "https://example.test".into(),
            headers: Default::default(),
            token: None,
            verbose: false,
            world: None,
        }
    }

    fn create_args() -> WorldCreateArgs {
        WorldCreateArgs {
            local_root: None,
            force_build: false,
            manifest_hash: None,
            from_world: None,
            upload_only: false,
            select: false,
            sync_secrets: false,
            world_id: None,
            universe_id: None,
            snapshot_ref: None,
            snapshot_height: None,
        }
    }

    #[test]
    fn created_world_id_accepts_local_response_shape() {
        let data = json!({
            "record": {
                "world_id": "11111111-1111-1111-1111-111111111111"
            }
        });

        let world_id = created_world_id(&data).expect("extract local world id");
        assert_eq!(world_id, "11111111-1111-1111-1111-111111111111");
    }

    #[test]
    fn created_world_id_accepts_hosted_response_shape() {
        let data = json!({
            "submission_id": "create-123",
            "submission_offset": 7,
            "world_id": "22222222-2222-2222-2222-222222222222",
            "effective_partition": 0
        });

        let world_id = created_world_id(&data).expect("extract hosted world id");
        assert_eq!(world_id, "22222222-2222-2222-2222-222222222222");
    }

    #[test]
    fn encode_state_key_query_uses_positional_string_by_default() {
        let args = WorldStateGetArgs {
            workflow: "sys/Workspace@1".into(),
            key: Some("workflow".into()),
            expand: false,
            key_json: None,
            key_bytes_b64: None,
            key_cbor_b64: None,
        };
        let encoded = encode_state_key_query(&args)
            .expect("encode key")
            .expect("key present");
        let bytes = BASE64_STANDARD.decode(encoded).expect("decode base64");
        let decoded: String = serde_cbor::from_slice(&bytes).expect("decode cbor string");
        assert_eq!(decoded, "workflow");
    }

    #[test]
    fn encode_state_key_query_rejects_multiple_key_sources() {
        let args = WorldStateGetArgs {
            workflow: "sys/Workspace@1".into(),
            key: Some("workflow".into()),
            expand: false,
            key_json: Some("\"workflow\"".into()),
            key_bytes_b64: None,
            key_cbor_b64: None,
        };
        assert!(encode_state_key_query(&args).is_err());
    }

    #[test]
    fn local_create_defaults_to_current_dir_when_air_dir_exists() {
        let _guard = cwd_test_lock().lock().expect("lock cwd test");
        let temp = TempDir::new().expect("temp dir");
        std::fs::create_dir_all(temp.path().join("air")).expect("create air dir");
        let previous = std::env::current_dir().expect("current dir");
        std::env::set_current_dir(temp.path()).expect("set current dir");
        let result = should_default_local_root(&local_target(), &create_args())
            .expect("decide default local root");
        std::env::set_current_dir(previous).expect("restore current dir");
        assert!(result);
    }

    #[test]
    fn remote_create_does_not_default_to_current_dir() {
        let _guard = cwd_test_lock().lock().expect("lock cwd test");
        let temp = TempDir::new().expect("temp dir");
        std::fs::create_dir_all(temp.path().join("air")).expect("create air dir");
        let previous = std::env::current_dir().expect("current dir");
        std::env::set_current_dir(temp.path()).expect("set current dir");
        let result = should_default_local_root(&remote_target(), &create_args())
            .expect("decide default local root");
        std::env::set_current_dir(previous).expect("restore current dir");
        assert!(!result);
    }

    #[test]
    fn declared_secret_binding_ids_resolves_refs_and_deduplicates() {
        let manifest = Manifest {
            air_version: "v1".into(),
            schemas: Vec::new(),
            modules: Vec::new(),
            effects: Vec::new(),
            effect_bindings: Vec::new(),
            caps: Vec::new(),
            policies: Vec::new(),
            secrets: vec![
                SecretEntry::Ref(NamedRef {
                    name: "llm/openai_api@1".into(),
                    hash: HashRef::new(
                        "sha256:1111111111111111111111111111111111111111111111111111111111111111",
                    )
                    .expect("hash"),
                }),
                SecretEntry::Decl(aos_air_types::SecretDecl {
                    alias: "anthropic".into(),
                    version: 1,
                    binding_id: "llm/anthropic_api".into(),
                    expected_digest: None,
                    policy: None,
                }),
                SecretEntry::Ref(NamedRef {
                    name: "llm/openai_api@1".into(),
                    hash: HashRef::new(
                        "sha256:2222222222222222222222222222222222222222222222222222222222222222",
                    )
                    .expect("hash"),
                }),
            ],
            defaults: None,
            module_bindings: Default::default(),
            routing: None,
        };
        let defs = vec![
            DefSecret {
                name: "llm/openai_api@1".into(),
                binding_id: "llm/openai_api".into(),
                expected_digest: None,
                allowed_caps: Vec::new(),
            },
            DefSecret {
                name: "llm/anthropic_api@1".into(),
                binding_id: "llm/anthropic_api".into(),
                expected_digest: None,
                allowed_caps: Vec::new(),
            },
        ];

        let binding_ids = declared_secret_binding_ids(&manifest, &defs);
        assert_eq!(
            binding_ids,
            vec![
                "llm/anthropic_api".to_string(),
                "llm/openai_api".to_string()
            ]
        );
    }

    #[test]
    fn parse_secret_binding_records_accepts_array_payload() {
        let records = parse_secret_binding_records(&json!([
            {
                "binding_id": "llm/openai_api",
                "source_kind": "node_secret_store",
                "latest_version": 1,
                "created_at_ns": 0,
                "updated_at_ns": 0,
                "status": "active"
            },
            {
                "binding_id": "llm/anthropic_api",
                "source_kind": "worker_env",
                "env_var": "ANTHROPIC_API_KEY",
                "created_at_ns": 0,
                "updated_at_ns": 0,
                "status": "disabled"
            }
        ]))
        .expect("parse records");

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].binding_id, "llm/openai_api");
        assert!(matches!(
            records[0].source_kind,
            SecretBindingSourceKind::NodeSecretStore
        ));
        assert!(matches!(records[0].status, SecretBindingStatus::Active));
        assert!(matches!(records[1].status, SecretBindingStatus::Disabled));
    }
}
