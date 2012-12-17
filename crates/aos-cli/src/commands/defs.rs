//! `aos defs` commands (list/get definitions from active manifest).

use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};

use crate::output::print_success;
use crate::opts::{Mode, WorldOpts, resolve_dirs};

use super::{should_use_control, try_control_client};

#[derive(Args, Debug)]
pub struct DefsArgs {
    #[command(subcommand)]
    pub cmd: DefsCommand,
}

#[derive(Subcommand, Debug)]
pub enum DefsCommand {
    /// Fetch a single def by name
    Get(DefsGetArgs),
    /// List defs from the active manifest
    Ls(DefsListArgs),
}

#[derive(Args, Debug)]
pub struct DefsGetArgs {
    /// Definition name (schema/module/plan/cap/effect/policy/secret)
    pub name: String,
}

#[derive(Args, Debug)]
pub struct DefsListArgs {
    /// Filter by def kind (repeatable)
    #[arg(long = "kind", value_enum)]
    pub kinds: Vec<DefKind>,

    /// Filter by name prefix
    #[arg(long)]
    pub prefix: Option<String>,
}

#[derive(Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum DefKind {
    Schema,
    Module,
    Plan,
    Cap,
    Effect,
    Policy,
    Secret,
}

pub async fn cmd_defs(opts: &WorldOpts, args: &DefsArgs) -> Result<()> {
    match &args.cmd {
        DefsCommand::Get(get) => defs_get(opts, get).await,
        DefsCommand::Ls(list) => defs_list(opts, list).await,
    }
}

async fn defs_get(opts: &WorldOpts, args: &DefsGetArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let resp = client.get_def("cli-def-get", &args.name).await?;
            if !resp.ok {
                anyhow::bail!(
                    "defs get failed: {}",
                    resp.error
                        .as_ref()
                        .map(|e| format!("{}: {}", e.code, e.message))
                        .unwrap_or_else(|| "unknown error".into())
                );
            }
            let result = resp.result.unwrap_or_default();
            let def = result.get("def").cloned().unwrap_or(serde_json::json!(null));
            let meta = result.get("meta").cloned();
            return print_success(opts, def, meta, vec![]);
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }

    // Fallback: load manifest assets and search locally
    let (def, warnings) = load_def_local(&dirs, &args.name)?;
    print_success(opts, def, None, warnings)
}

async fn defs_list(opts: &WorldOpts, args: &DefsListArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let kinds: Option<Vec<String>> =
                if args.kinds.is_empty() {
                    None
                } else {
                    Some(args.kinds.iter().map(|k| def_kind_str(k).to_string()).collect())
                };
            let kind_refs: Option<Vec<&str>> =
                kinds.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());
            let resp = client
                .list_defs(
                    "cli-def-list",
                    kind_refs.as_deref(),
                    args.prefix.as_deref(),
                )
                .await?;
            if !resp.ok {
                anyhow::bail!(
                    "defs list failed: {}",
                    resp.error
                        .as_ref()
                        .map(|e| format!("{}: {}", e.code, e.message))
                        .unwrap_or_else(|| "unknown error".into())
                );
            }
            let result = resp.result.unwrap_or_default();
            let defs = result.get("defs").cloned().unwrap_or(serde_json::json!([]));
            let meta = result.get("meta").cloned();
            return print_success(opts, defs, meta, vec![]);
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }

    // Fallback: read defs from manifest assets
    let (defs, warnings) = list_defs_local(&dirs, &args.kinds, args.prefix.as_deref())?;
    print_success(opts, defs, None, warnings)
}

fn load_def_local(
    dirs: &crate::opts::ResolvedDirs,
    name: &str,
) -> Result<(serde_json::Value, Vec<String>)> {
    use std::sync::Arc;
    let mut warnings = vec![];
    let store = Arc::new(aos_store::FsStore::open(&dirs.store_root)?);
    let loaded = aos_host::manifest_loader::load_from_assets(store, &dirs.air_dir)?
        .ok_or_else(|| anyhow::anyhow!("no manifest found in {}", dirs.air_dir.display()))?;
    let name_val: aos_air_types::Name = name.to_string();

    macro_rules! try_map {
        ($map:expr) => {
            if let Some(def) = $map.get(&name_val) {
                return Ok((serde_json::to_value(def)?, warnings));
            }
        };
    }

    try_map!(loaded.schemas);
    try_map!(loaded.modules);
    try_map!(loaded.plans);
    try_map!(loaded.caps);
    try_map!(loaded.effects);
    try_map!(loaded.policies);
    warnings.push(format!("def '{}' not found locally", name));
    Ok((serde_json::json!(null), warnings))
}

fn list_defs_local(
    dirs: &crate::opts::ResolvedDirs,
    kinds: &[DefKind],
    prefix: Option<&str>,
) -> Result<(serde_json::Value, Vec<String>)> {
    use std::sync::Arc;
    let store = Arc::new(aos_store::FsStore::open(&dirs.store_root)?);
    let loaded = aos_host::manifest_loader::load_from_assets(store, &dirs.air_dir)?
        .ok_or_else(|| anyhow::anyhow!("no manifest found in {}", dirs.air_dir.display()))?;

    let mut defs = Vec::new();
    let prefix_match = |s: &aos_air_types::Name| {
        if let Some(p) = prefix {
            s.as_str().starts_with(p)
        } else {
            true
        }
    };
    let allow_kind = |k: DefKind| kinds.is_empty() || kinds.contains(&k);

    if allow_kind(DefKind::Schema) {
        for d in loaded.schemas.values() {
            if prefix_match(&d.name) {
                defs.push(serde_json::json!({ "kind": "schema", "name": d.name }));
            }
        }
    }
    if allow_kind(DefKind::Module) {
        for d in loaded.modules.values() {
            if prefix_match(&d.name) {
                defs.push(serde_json::json!({ "kind": "module", "name": d.name }));
            }
        }
    }
    if allow_kind(DefKind::Plan) {
        for d in loaded.plans.values() {
            if prefix_match(&d.name) {
                defs.push(serde_json::json!({ "kind": "plan", "name": d.name }));
            }
        }
    }
    if allow_kind(DefKind::Cap) {
        for d in loaded.caps.values() {
            if prefix_match(&d.name) {
                defs.push(serde_json::json!({ "kind": "cap", "name": d.name }));
            }
        }
    }
    if allow_kind(DefKind::Effect) {
        for d in loaded.effects.values() {
            if prefix_match(&d.name) {
                defs.push(serde_json::json!({ "kind": "effect", "name": d.name }));
            }
        }
    }
    if allow_kind(DefKind::Policy) {
        for d in loaded.policies.values() {
            if prefix_match(&d.name) {
                defs.push(serde_json::json!({ "kind": "policy", "name": d.name }));
            }
        }
    }

    Ok((serde_json::json!(defs), vec![]))
}

fn def_kind_str(kind: &DefKind) -> &'static str {
    match kind {
        DefKind::Schema => "schema",
        DefKind::Module => "module",
        DefKind::Plan => "plan",
        DefKind::Cap => "cap",
        DefKind::Effect => "effect",
        DefKind::Policy => "policy",
        DefKind::Secret => "secret",
    }
}
