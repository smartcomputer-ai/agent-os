//! `aos plans` commands for plan-pack import reuse checks and scaffolding.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use aos_air_types::{
    AirNode, DefPlan, EffectKind, Expr, ExprConst, ExprOrValue, Manifest, PlanStepEmitEffect,
    PlanStepKind, Trigger, ValueLiteral,
};
use aos_cbor::Hash;
use aos_host::manifest_loader;
use aos_store::FsStore;
use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::{Value, json};
use walkdir::WalkDir;

use crate::opts::{WorldOpts, resolve_dirs};
use crate::output::print_success;

use super::sync::{ResolvedAirImport, load_sync_config, resolve_air_sources};

const ZERO_HASH_SENTINEL: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Args, Debug)]
pub struct PlansArgs {
    #[command(subcommand)]
    pub cmd: PlansCommand,
}

#[derive(Subcommand, Debug)]
pub enum PlansCommand {
    /// Validate imported plan packs and consumer wiring.
    Check(PlansCheckArgs),
    /// Generate consumption scaffolds for an imported plan pack.
    Scaffold(PlansScaffoldArgs),
}

#[derive(Args, Debug)]
pub struct PlansCheckArgs {
    /// Sync config path (defaults to <world>/aos.sync.json)
    #[arg(long)]
    pub map: Option<PathBuf>,

    /// Treat warnings as failure.
    #[arg(long, default_value_t = false)]
    pub fail_on_warning: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ScaffoldProfile {
    Turnkey,
    #[value(name = "composable-core")]
    ComposableCore,
}

#[derive(Args, Debug)]
pub struct PlansScaffoldArgs {
    /// Sync config path (defaults to <world>/aos.sync.json)
    #[arg(long)]
    pub map: Option<PathBuf>,

    /// Pack name (directory name under plan-packs/ or import root basename)
    #[arg(long)]
    pub pack: String,

    /// Consumption profile to scaffold.
    #[arg(long, value_enum)]
    pub profile: ScaffoldProfile,

    /// Target plan name (required when multiple candidates exist)
    #[arg(long)]
    pub plan: Option<String>,

    /// Trigger event schema override (turnkey only; defaults to entry plan input)
    #[arg(long)]
    pub trigger_event: Option<String>,

    /// Output scaffold file path (default: <air>/scaffolds/plan-pack-<pack>-<profile>.json)
    #[arg(long)]
    pub out: Option<PathBuf>,

    /// Print scaffold JSON without writing files.
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,

    /// Overwrite existing scaffold files.
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

pub async fn cmd_plans(opts: &WorldOpts, args: &PlansArgs) -> Result<()> {
    match &args.cmd {
        PlansCommand::Check(check) => cmd_plans_check(opts, check).await,
        PlansCommand::Scaffold(scaffold) => cmd_plans_scaffold(opts, scaffold).await,
    }
}

async fn cmd_plans_check(opts: &WorldOpts, args: &PlansCheckArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let (map_path, config) = load_sync_config(&dirs.world, args.map.as_deref())?;
    let map_root = map_path.parent().unwrap_or(&dirs.world);

    let air_sources = resolve_air_sources(
        &dirs.world,
        map_root,
        &config,
        &dirs.air_dir,
        &dirs.workflow_dir,
    )?;

    let mut warnings = air_sources.warnings.clone();
    let mut errors = Vec::new();

    let store = Arc::new(FsStore::open(&dirs.store_root).context("open store")?);
    let loaded = manifest_loader::load_from_assets_with_imports(
        store,
        &air_sources.air_dir,
        &air_sources.import_dirs,
    )
    .with_context(|| format!("load AIR assets from {}", air_sources.air_dir.display()))?
    .ok_or_else(|| anyhow::anyhow!("no manifest found in {}", air_sources.air_dir.display()))?;

    let packs = analyze_import_packs(&air_sources.imports)?;

    for pack in &packs {
        if !pack.unknown_role_plans.is_empty() {
            errors.push(format!(
                "pack '{}' has plans outside role conventions: {}",
                pack.pack,
                pack.unknown_role_plans.join(", ")
            ));
        }
        if pack.has_plans && !pack.turnkey_capable && !pack.composable_core_capable {
            errors.push(format!(
                "pack '{}' has plans but no entry_/core_ roles; profile inference is ambiguous",
                pack.pack
            ));
        }
    }

    let grant_names: HashSet<&str> = loaded
        .manifest
        .defaults
        .as_ref()
        .map(|defaults| {
            defaults
                .cap_grants
                .iter()
                .map(|g| g.name.as_str())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();

    let manifest_plan_names: HashSet<&str> = loaded
        .manifest
        .plans
        .iter()
        .map(|p| p.name.as_str())
        .collect();

    let mut imported_entry_inputs = HashMap::new();
    for pack in &packs {
        for contract in &pack.contracts {
            if contract.role == PlanRole::Entry {
                imported_entry_inputs.insert(contract.name.clone(), contract.input.clone());
            }
            if manifest_plan_names.contains(contract.name.as_str()) {
                for cap in &contract.required_caps {
                    if !grant_names.contains(cap.as_str()) {
                        errors.push(format!(
                            "imported plan '{}' requires cap grant '{}' but manifest.defaults.cap_grants is missing it",
                            contract.name, cap
                        ));
                    }
                }
            }
        }
    }

    let mut turnkey_bound_plans: HashSet<String> = HashSet::new();
    for trigger in &loaded.manifest.triggers {
        if let Some(expected_input) = imported_entry_inputs.get(trigger.plan.as_str()) {
            if trigger.event.as_str() == expected_input {
                turnkey_bound_plans.insert(trigger.plan.clone());
            } else {
                errors.push(format!(
                    "turnkey trigger mismatch for entry plan '{}': trigger event '{}' must equal input schema '{}'",
                    trigger.plan,
                    trigger.event,
                    expected_input
                ));
            }
        }
    }

    let local_plans = collect_plan_hashes(&air_sources.air_dir, false)?;
    let import_plans = collect_import_plan_hashes(&packs);
    for (plan_name, local_hash) in &local_plans {
        if let Some(import_hash) = import_plans.get(plan_name) {
            if import_hash == local_hash {
                if turnkey_bound_plans.contains(plan_name) {
                    errors.push(format!(
                        "turnkey consumer has redundant local copy of imported plan '{}' (hash {})",
                        plan_name, local_hash
                    ));
                } else {
                    warnings.push(format!(
                        "local plan '{}' duplicates imported plan body (hash {}); consider removing local copy",
                        plan_name,
                        local_hash
                    ));
                }
            }
        }
    }
    warnings.extend(collect_runtime_hardening_lints(
        &loaded.manifest,
        &loaded.plans,
    ));

    let data = json!({
        "map": map_path.display().to_string(),
        "air_dir": air_sources.air_dir.display().to_string(),
        "imports": packs,
        "errors": errors,
    });

    let has_errors = data
        .get("errors")
        .and_then(|v| v.as_array())
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    let has_warnings = !warnings.is_empty();

    print_success(opts, data, None, warnings)?;

    if has_errors || (args.fail_on_warning && has_warnings) {
        anyhow::bail!("plans check failed")
    }
    Ok(())
}

async fn cmd_plans_scaffold(opts: &WorldOpts, args: &PlansScaffoldArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let (map_path, config) = load_sync_config(&dirs.world, args.map.as_deref())?;
    let map_root = map_path.parent().unwrap_or(&dirs.world);

    let air_sources = resolve_air_sources(
        &dirs.world,
        map_root,
        &config,
        &dirs.air_dir,
        &dirs.workflow_dir,
    )?;

    let packs = analyze_import_packs(&air_sources.imports)?;
    let pack = packs
        .iter()
        .find(|pack| pack.pack == args.pack)
        .ok_or_else(|| anyhow::anyhow!("imported plan pack '{}' not found", args.pack))?;

    let target = match args.profile {
        ScaffoldProfile::Turnkey => {
            select_plan_for_role(&pack.contracts, PlanRole::Entry, args.plan.as_deref())?
        }
        ScaffoldProfile::ComposableCore => {
            select_plan_for_role(&pack.contracts, PlanRole::Core, args.plan.as_deref())?
        }
    };

    let scaffold = match args.profile {
        ScaffoldProfile::Turnkey => {
            build_turnkey_scaffold(pack, target, args.trigger_event.as_deref())?
        }
        ScaffoldProfile::ComposableCore => build_composable_scaffold(pack, target)?,
    };

    let default_out = match args.profile {
        ScaffoldProfile::Turnkey => air_sources.air_dir.join("scaffolds").join(format!(
            "plan-pack-{}-turnkey.json",
            sanitize_id(&pack.pack)
        )),
        ScaffoldProfile::ComposableCore => air_sources.air_dir.join("scaffolds").join(format!(
            "plan-pack-{}-composable-core.json",
            sanitize_id(&pack.pack)
        )),
    };
    let out_path = args
        .out
        .as_ref()
        .map(|path| resolve_cli_path(&dirs.world, path))
        .unwrap_or(default_out);

    let mut warnings = air_sources.warnings.clone();

    if args.dry_run {
        return print_success(
            opts,
            json!({
                "pack": pack.pack,
                "profile": match args.profile {
                    ScaffoldProfile::Turnkey => "turnkey",
                    ScaffoldProfile::ComposableCore => "composable-core",
                },
                "target_plan": target.name,
                "out": out_path.display().to_string(),
                "scaffold": scaffold,
            }),
            None,
            warnings,
        );
    }

    if out_path.exists() && !args.force {
        anyhow::bail!(
            "scaffold output '{}' already exists (use --force to overwrite)",
            out_path.display()
        );
    }
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create scaffold directory {}", parent.display()))?;
    }
    fs::write(&out_path, serde_json::to_string_pretty(&scaffold)?)
        .with_context(|| format!("write scaffold {}", out_path.display()))?;

    let mut written_files = vec![out_path.display().to_string()];

    if let Some(wrapper) = scaffold.get("wrapper_plan_template") {
        let flow = flow_name_from_plan(target.name.as_str());
        let wrapper_path = air_sources
            .air_dir
            .join("plans")
            .join(format!("adapter_{}_template.air.json", flow));
        if wrapper_path.exists() && !args.force {
            warnings.push(format!(
                "wrapper template not written because '{}' already exists (use --force)",
                wrapper_path.display()
            ));
        } else {
            if let Some(parent) = wrapper_path.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("create wrapper template directory {}", parent.display())
                })?;
            }
            fs::write(&wrapper_path, serde_json::to_string_pretty(wrapper)?)
                .with_context(|| format!("write wrapper template {}", wrapper_path.display()))?;
            written_files.push(wrapper_path.display().to_string());
        }
    }

    print_success(
        opts,
        json!({
            "pack": pack.pack,
            "profile": match args.profile {
                ScaffoldProfile::Turnkey => "turnkey",
                ScaffoldProfile::ComposableCore => "composable-core",
            },
            "target_plan": target.name,
            "written": written_files,
        }),
        None,
        warnings,
    )
}

fn collect_runtime_hardening_lints(
    manifest: &Manifest,
    plans: &HashMap<String, DefPlan>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    warnings.extend(correlated_wait_lints(&manifest.triggers, plans));
    warnings.extend(long_wait_timeout_lints(plans));
    warnings.extend(idempotency_key_lints(plans));
    warnings.sort();
    warnings
}

fn correlated_wait_lints(triggers: &[Trigger], plans: &HashMap<String, DefPlan>) -> Vec<String> {
    let mut warnings = Vec::new();
    for trigger in triggers {
        let Some(correlate_by) = trigger.correlate_by.as_ref() else {
            continue;
        };
        let reachable = reachable_plans(trigger.plan.as_str(), plans);
        let mut awaited_steps = Vec::new();
        let mut correlated_steps = 0usize;

        for plan_name in reachable {
            let Some(plan) = plans.get(&plan_name) else {
                continue;
            };
            for step in &plan.steps {
                let PlanStepKind::AwaitEvent(await_event) = &step.kind else {
                    continue;
                };
                let correlated = await_event_uses_correlation(
                    await_event.where_clause.as_ref(),
                    correlate_by.as_str(),
                );
                if correlated {
                    correlated_steps += 1;
                }
                awaited_steps.push((plan_name.clone(), step.id.clone(), correlated));
            }
        }

        if awaited_steps.is_empty() {
            continue;
        }

        if correlated_steps == 0 {
            warnings.push(format!(
                "trigger '{}' -> '{}' declares correlate_by='{}' but no reachable await_event.where predicate matches @event.{} against correlation source (@var:correlation_id, @var:{}, or @plan.input.{})",
                trigger.event, trigger.plan, correlate_by, correlate_by, correlate_by, correlate_by
            ));
            continue;
        }

        for (plan_name, step_id, correlated) in awaited_steps {
            if correlated {
                continue;
            }
            warnings.push(format!(
                "plan '{}' step '{}' awaits events on correlated trigger path ('{}' -> '{}') without pairing @event.{} with a correlation source (@var:correlation_id, @var:{}, or @plan.input.{})",
                plan_name, step_id, trigger.event, trigger.plan, correlate_by, correlate_by, correlate_by
            ));
        }
    }
    warnings
}

fn long_wait_timeout_lints(plans: &HashMap<String, DefPlan>) -> Vec<String> {
    const LONG_WAIT_THRESHOLD: usize = 2;
    let mut warnings = Vec::new();

    let mut plan_names = plans.keys().cloned().collect::<Vec<_>>();
    plan_names.sort();
    for plan_name in plan_names {
        let Some(plan) = plans.get(&plan_name) else {
            continue;
        };
        let wait_count = plan_wait_count(plan);
        if wait_count < LONG_WAIT_THRESHOLD || plan_has_timeout_branch(plan) {
            continue;
        }
        warnings.push(format!(
            "plan '{}' has {} wait steps (await_event/await_receipt/await_plan/await_plans_all) but no explicit timeout branch signal (timer.set or timeout/deadline markers)",
            plan.name, wait_count
        ));
    }
    warnings
}

fn idempotency_key_lints(plans: &HashMap<String, DefPlan>) -> Vec<String> {
    let mut warnings = Vec::new();

    let mut plan_names = plans.keys().cloned().collect::<Vec<_>>();
    plan_names.sort();
    for plan_name in plan_names {
        let Some(plan) = plans.get(&plan_name) else {
            continue;
        };
        for step in &plan.steps {
            let PlanStepKind::EmitEffect(emit) = &step.kind else {
                continue;
            };
            if emit.idempotency_key.is_none() && !emit_effect_is_read_only(emit) {
                warnings.push(format!(
                    "plan '{}' step '{}' emits '{}' without idempotency_key (recommended for non-read effects)",
                    plan.name,
                    step.id,
                    emit.kind
                ));
            }
        }
    }

    warnings
}

fn reachable_plans(root: &str, plans: &HashMap<String, DefPlan>) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![root.to_string()];

    while let Some(plan_name) = stack.pop() {
        if !seen.insert(plan_name.clone()) {
            continue;
        }
        let Some(plan) = plans.get(&plan_name) else {
            continue;
        };
        for step in &plan.steps {
            match &step.kind {
                PlanStepKind::SpawnPlan(spawn) => stack.push(spawn.plan.clone()),
                PlanStepKind::SpawnForEach(spawn) => stack.push(spawn.plan.clone()),
                _ => {}
            }
        }
    }

    seen
}

fn await_event_uses_correlation(where_clause: Option<&Expr>, correlate_by: &str) -> bool {
    let Some(where_clause) = where_clause else {
        return false;
    };
    let event_ref = format!("@event.{correlate_by}");
    let event_ref_prefix = format!("{event_ref}.");
    let has_event_ref = expr_any_ref(where_clause, &|reference| {
        reference == event_ref || reference.starts_with(event_ref_prefix.as_str())
    });
    let plan_input_ref = format!("@plan.input.{correlate_by}");
    let plan_input_ref_prefix = format!("{plan_input_ref}.");
    let correlate_var_ref = format!("@var:{correlate_by}");
    let correlate_var_ref_prefix = format!("{correlate_var_ref}.");
    let has_correlation_source = expr_any_ref(where_clause, &|reference| {
        reference == "@var:correlation_id"
            || reference.starts_with("@var:correlation_id.")
            || reference == plan_input_ref
            || reference.starts_with(plan_input_ref_prefix.as_str())
            || reference == correlate_var_ref
            || reference.starts_with(correlate_var_ref_prefix.as_str())
    });
    has_event_ref && has_correlation_source
}

fn plan_wait_count(plan: &DefPlan) -> usize {
    plan.steps
        .iter()
        .filter(|step| {
            matches!(
                step.kind,
                PlanStepKind::AwaitReceipt(_)
                    | PlanStepKind::AwaitEvent(_)
                    | PlanStepKind::AwaitPlan(_)
                    | PlanStepKind::AwaitPlansAll(_)
            )
        })
        .count()
}

fn plan_has_timeout_branch(plan: &DefPlan) -> bool {
    if has_timeout_token(plan.name.as_str()) {
        return true;
    }
    for step in &plan.steps {
        if has_timeout_token(step.id.as_str()) {
            return true;
        }
        match &step.kind {
            PlanStepKind::RaiseEvent(raise) => {
                if has_timeout_token(raise.event.as_str())
                    || expr_or_value_has_timeout_token(&raise.value)
                {
                    return true;
                }
            }
            PlanStepKind::EmitEffect(emit) => {
                if emit.kind.as_str() == EffectKind::TIMER_SET
                    || has_timeout_token(emit.kind.as_str())
                    || expr_or_value_has_timeout_token(&emit.params)
                    || emit
                        .idempotency_key
                        .as_ref()
                        .map(expr_or_value_has_timeout_token)
                        .unwrap_or(false)
                {
                    return true;
                }
            }
            PlanStepKind::AwaitReceipt(await_receipt) => {
                if expr_has_timeout_token(&await_receipt.for_expr) {
                    return true;
                }
            }
            PlanStepKind::AwaitEvent(await_event) => {
                if has_timeout_token(await_event.event.as_str())
                    || await_event
                        .where_clause
                        .as_ref()
                        .map(expr_has_timeout_token)
                        .unwrap_or(false)
                {
                    return true;
                }
            }
            PlanStepKind::SpawnPlan(spawn) => {
                if has_timeout_token(spawn.plan.as_str())
                    || expr_or_value_has_timeout_token(&spawn.input)
                {
                    return true;
                }
            }
            PlanStepKind::AwaitPlan(await_plan) => {
                if expr_has_timeout_token(&await_plan.for_expr) {
                    return true;
                }
            }
            PlanStepKind::SpawnForEach(spawn) => {
                if has_timeout_token(spawn.plan.as_str())
                    || expr_or_value_has_timeout_token(&spawn.inputs)
                {
                    return true;
                }
            }
            PlanStepKind::AwaitPlansAll(await_all) => {
                if expr_has_timeout_token(&await_all.handles) {
                    return true;
                }
            }
            PlanStepKind::Assign(assign) => {
                if expr_or_value_has_timeout_token(&assign.expr) {
                    return true;
                }
            }
            PlanStepKind::End(end) => {
                if end
                    .result
                    .as_ref()
                    .map(expr_or_value_has_timeout_token)
                    .unwrap_or(false)
                {
                    return true;
                }
            }
        }
    }
    for edge in &plan.edges {
        if edge
            .when
            .as_ref()
            .map(expr_has_timeout_token)
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

fn emit_effect_is_read_only(emit: &PlanStepEmitEffect) -> bool {
    match emit.kind.as_str() {
        EffectKind::BLOB_GET
        | EffectKind::INTROSPECT_MANIFEST
        | EffectKind::INTROSPECT_WORKFLOW_STATE
        | EffectKind::INTROSPECT_JOURNAL_HEAD
        | EffectKind::INTROSPECT_LIST_CELLS
        | EffectKind::WORKSPACE_RESOLVE
        | EffectKind::WORKSPACE_LIST
        | EffectKind::WORKSPACE_READ_REF
        | EffectKind::WORKSPACE_READ_BYTES
        | EffectKind::WORKSPACE_DIFF
        | EffectKind::WORKSPACE_ANNOTATIONS_GET => true,
        EffectKind::HTTP_REQUEST => http_method_is_read_only(&emit.params),
        _ => {
            emit.kind.as_str().starts_with("introspect.")
                || emit.kind.as_str().starts_with("query.")
        }
    }
}

fn http_method_is_read_only(params: &ExprOrValue) -> bool {
    method_from_expr_or_value(params)
        .map(|method| {
            matches!(
                method.to_ascii_uppercase().as_str(),
                "GET" | "HEAD" | "OPTIONS"
            )
        })
        .unwrap_or(false)
}

fn method_from_expr_or_value(params: &ExprOrValue) -> Option<&str> {
    match params {
        ExprOrValue::Expr(expr) => method_from_expr(expr),
        ExprOrValue::Literal(value) => method_from_value(value),
        ExprOrValue::Json(value) => method_from_json(value),
    }
}

fn method_from_expr(expr: &Expr) -> Option<&str> {
    let Expr::Record(record) = expr else {
        return None;
    };
    let value = record.record.get("method")?;
    let Expr::Const(ExprConst::Text { text }) = value else {
        return None;
    };
    Some(text.as_str())
}

fn method_from_value(value: &ValueLiteral) -> Option<&str> {
    let ValueLiteral::Record(record) = value else {
        return None;
    };
    let ValueLiteral::Text(method) = record.record.get("method")? else {
        return None;
    };
    Some(method.text.as_str())
}

fn method_from_json(value: &Value) -> Option<&str> {
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        return Some(method);
    }
    value
        .get("record")
        .and_then(Value::as_object)
        .and_then(|record| record.get("method"))
        .and_then(|method| {
            method
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| method.as_str())
        })
}

fn has_timeout_token(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("timeout") || text.contains("deadline") || text.contains("ttl")
}

fn expr_or_value_has_timeout_token(value: &ExprOrValue) -> bool {
    match value {
        ExprOrValue::Expr(expr) => expr_has_timeout_token(expr),
        ExprOrValue::Literal(value) => value_has_timeout_token(value),
        ExprOrValue::Json(value) => json_has_timeout_token(value),
    }
}

fn expr_has_timeout_token(expr: &Expr) -> bool {
    match expr {
        Expr::Ref(reference) => has_timeout_token(reference.reference.as_str()),
        Expr::Const(ExprConst::Text { text }) => has_timeout_token(text.as_str()),
        Expr::Const(_) => false,
        Expr::Op(op) => op.args.iter().any(expr_has_timeout_token),
        Expr::Record(record) => record.record.values().any(expr_has_timeout_token),
        Expr::List(list) => list.list.iter().any(expr_has_timeout_token),
        Expr::Set(set) => set.set.iter().any(expr_has_timeout_token),
        Expr::Map(map) => map.map.iter().any(|entry| {
            expr_has_timeout_token(&entry.key) || expr_has_timeout_token(&entry.value)
        }),
        Expr::Variant(variant) => variant
            .variant
            .value
            .as_deref()
            .map(expr_has_timeout_token)
            .unwrap_or(false),
    }
}

fn value_has_timeout_token(value: &ValueLiteral) -> bool {
    match value {
        ValueLiteral::Text(text) => has_timeout_token(text.text.as_str()),
        ValueLiteral::List(list) => list.list.iter().any(value_has_timeout_token),
        ValueLiteral::Set(set) => set.set.iter().any(value_has_timeout_token),
        ValueLiteral::Map(map) => map.map.iter().any(|entry| {
            value_has_timeout_token(&entry.key) || value_has_timeout_token(&entry.value)
        }),
        ValueLiteral::Record(record) => record.record.values().any(value_has_timeout_token),
        ValueLiteral::Variant(variant) => variant
            .value
            .as_deref()
            .map(value_has_timeout_token)
            .unwrap_or(false),
        _ => false,
    }
}

fn json_has_timeout_token(value: &Value) -> bool {
    match value {
        Value::String(text) => has_timeout_token(text.as_str()),
        Value::Array(items) => items.iter().any(json_has_timeout_token),
        Value::Object(map) => map.values().any(json_has_timeout_token),
        _ => false,
    }
}

fn expr_any_ref<F>(expr: &Expr, predicate: &F) -> bool
where
    F: Fn(&str) -> bool,
{
    match expr {
        Expr::Ref(reference) => predicate(reference.reference.as_str()),
        Expr::Const(_) => false,
        Expr::Op(op) => op.args.iter().any(|arg| expr_any_ref(arg, predicate)),
        Expr::Record(record) => record
            .record
            .values()
            .any(|value| expr_any_ref(value, predicate)),
        Expr::List(list) => list.list.iter().any(|value| expr_any_ref(value, predicate)),
        Expr::Set(set) => set.set.iter().any(|value| expr_any_ref(value, predicate)),
        Expr::Map(map) => map.map.iter().any(|entry| {
            expr_any_ref(&entry.key, predicate) || expr_any_ref(&entry.value, predicate)
        }),
        Expr::Variant(variant) => variant
            .variant
            .value
            .as_deref()
            .map(|value| expr_any_ref(value, predicate))
            .unwrap_or(false),
    }
}

fn select_plan_for_role<'a>(
    contracts: &'a [PlanContract],
    role: PlanRole,
    selected: Option<&str>,
) -> Result<&'a PlanContract> {
    let candidates: Vec<&PlanContract> = contracts.iter().filter(|c| c.role == role).collect();
    if let Some(name) = selected {
        return candidates
            .into_iter()
            .find(|c| c.name == name)
            .ok_or_else(|| anyhow::anyhow!("plan '{}' not found for requested profile", name));
    }
    if candidates.is_empty() {
        anyhow::bail!("no plans available for requested profile")
    }
    if candidates.len() > 1 {
        let names = candidates
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "multiple plans match requested profile ({names}); pass --plan to choose one"
        );
    }
    Ok(candidates[0])
}

fn build_turnkey_scaffold(
    pack: &PackReport,
    target: &PlanContract,
    trigger_override: Option<&str>,
) -> Result<Value> {
    let trigger_event = trigger_override.unwrap_or(target.input.as_str());
    let policy_name = format!(
        "{}.policy/{}_turnkey@1",
        sanitize_namespace(&pack.pack),
        sanitize_id(&flow_name_from_plan(target.name.as_str()))
    );

    let cap_grants: Vec<Value> = target
        .required_caps
        .iter()
        .map(|cap| {
            json!({
                "name": cap,
                "cap": "TODO/CapDefinition@1",
                "params": { "record": {} }
            })
        })
        .collect();

    let rules: Vec<Value> = target
        .allowed_effects
        .iter()
        .map(|effect| {
            json!({
                "when": {
                    "effect_kind": effect,
                    "origin_kind": "workflow",
                    "origin_name": target.name,
                },
                "decision": "allow"
            })
        })
        .collect();

    Ok(json!({
        "kind": "plan-pack-scaffold",
        "profile": "turnkey",
        "pack": pack.pack,
        "target_plan": target.name,
        "contract": target,
        "turnkey_trigger_rule": {
            "required_event": target.input,
            "configured_event": trigger_event,
        },
        "manifest_snippet": {
            "plans": [
                { "name": target.name, "hash": ZERO_HASH_SENTINEL }
            ],
            "triggers": [
                { "event": trigger_event, "plan": target.name }
            ],
            "defaults": {
                "policy": policy_name,
                "cap_grants": cap_grants,
            }
        },
        "policy_def": {
            "$kind": "defpolicy",
            "name": policy_name,
            "rules": rules,
        }
    }))
}

fn build_composable_scaffold(pack: &PackReport, target: &PlanContract) -> Result<Value> {
    let flow = flow_name_from_plan(target.name.as_str());
    let adapter_name = format!("app/adapter_{}@1", sanitize_id(&flow));
    let local_input = format!("app/{}Requested@1", to_pascal_case(&flow));
    let policy_name = format!(
        "{}.policy/{}_core@1",
        sanitize_namespace(&pack.pack),
        sanitize_id(&flow)
    );

    let cap_grants: Vec<Value> = target
        .required_caps
        .iter()
        .map(|cap| {
            json!({
                "name": cap,
                "cap": "TODO/CapDefinition@1",
                "params": { "record": {} }
            })
        })
        .collect();

    let rules: Vec<Value> = target
        .allowed_effects
        .iter()
        .map(|effect| {
            json!({
                "when": {
                    "effect_kind": effect,
                    "origin_kind": "workflow",
                    "origin_name": target.name,
                },
                "decision": "allow"
            })
        })
        .collect();

    let wrapper_plan = json!({
        "$kind": "defplan",
        "name": adapter_name,
        "input": local_input,
        "locals": {
            "core_input": target.input,
        },
        "steps": [
            {
                "id": "map_to_core",
                "op": "assign",
                "expr": { "ref": "@plan.input" },
                "bind": { "as": "core_input" }
            },
            {
                "id": "emit_core_event",
                "op": "raise_event",
                "event": target.input,
                "value": { "ref": "@var:core_input" }
            },
            {
                "id": "finish",
                "op": "end"
            }
        ],
        "edges": [
            { "from": "map_to_core", "to": "emit_core_event" },
            { "from": "emit_core_event", "to": "finish" }
        ],
        "required_caps": [],
        "allowed_effects": []
    });

    Ok(json!({
        "kind": "plan-pack-scaffold",
        "profile": "composable-core",
        "pack": pack.pack,
        "target_plan": target.name,
        "contract": target,
        "manifest_snippet": {
            "plans": [
                { "name": target.name, "hash": ZERO_HASH_SENTINEL },
                { "name": adapter_name, "hash": ZERO_HASH_SENTINEL }
            ],
            "triggers": [
                { "event": local_input, "plan": adapter_name },
                { "event": target.input, "plan": target.name }
            ],
            "defaults": {
                "policy": policy_name,
                "cap_grants": cap_grants,
            }
        },
        "policy_def": {
            "$kind": "defpolicy",
            "name": policy_name,
            "rules": rules,
        },
        "wrapper_plan_template": wrapper_plan,
        "notes": [
            "Replace wrapper input schema and mapping expression with app-local envelope projection.",
            "Keep cap grants and policy world-local; imported core plan slot names are stable API.",
            "If you keep local envelopes, wrap output events similarly in app-local adapter plans."
        ]
    }))
}

fn resolve_cli_path(world_root: &Path, path: &Path) -> PathBuf {
    if path.is_relative() {
        world_root.join(path)
    } else {
        path.to_path_buf()
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PlanRole {
    Entry,
    Core,
    Adapter,
    Internal,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
struct PlanContract {
    name: String,
    role: PlanRole,
    input: String,
    output: Option<String>,
    emitted_events: Vec<String>,
    required_caps: Vec<String>,
    allowed_effects: Vec<String>,
    hash: String,
}

#[derive(Debug, Clone, Serialize)]
struct PackReport {
    pack: String,
    root: String,
    has_plans: bool,
    turnkey_capable: bool,
    composable_core_capable: bool,
    contracts: Vec<PlanContract>,
    unknown_role_plans: Vec<String>,
}

fn analyze_import_packs(imports: &[ResolvedAirImport]) -> Result<Vec<PackReport>> {
    let mut reports = Vec::new();
    for import in imports {
        let plans = collect_plan_nodes(&import.root, true)?;
        let pack = infer_pack_name(import);

        let mut unknown_role_plans = Vec::new();
        let mut contracts = Vec::new();
        let mut turnkey = false;
        let mut composable = false;

        for (plan, hash) in plans {
            let role = classify_role(plan.name.as_str());
            if role == PlanRole::Unknown {
                unknown_role_plans.push(plan.name.clone());
            }
            if role == PlanRole::Entry {
                turnkey = true;
            }
            if role == PlanRole::Core {
                composable = true;
            }
            contracts.push(extract_contract(&plan, role, hash));
        }

        contracts.sort_by(|a, b| a.name.cmp(&b.name));
        unknown_role_plans.sort();

        reports.push(PackReport {
            pack,
            root: import.root.display().to_string(),
            has_plans: !contracts.is_empty(),
            turnkey_capable: turnkey,
            composable_core_capable: composable,
            contracts,
            unknown_role_plans,
        });
    }
    reports.sort_by(|a, b| a.pack.cmp(&b.pack));
    Ok(reports)
}

fn collect_import_plan_hashes(packs: &[PackReport]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pack in packs {
        for contract in &pack.contracts {
            map.entry(contract.name.clone())
                .or_insert_with(|| contract.hash.clone());
        }
    }
    map
}

fn collect_plan_hashes(root: &Path, include_root: bool) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    for (plan, hash) in collect_plan_nodes(root, include_root)? {
        map.insert(plan.name.clone(), hash);
    }
    Ok(map)
}

fn collect_plan_nodes(root: &Path, include_root: bool) -> Result<Vec<(DefPlan, String)>> {
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut out: Vec<(DefPlan, String)> = Vec::new();
    for dir in asset_search_dirs(root, include_root)? {
        for path in collect_json_files(&dir)? {
            for node in parse_air_nodes(&path)
                .with_context(|| format!("parse AIR nodes from {}", path.display()))?
            {
                if let AirNode::Defplan(plan) = node {
                    let hash = Hash::of_cbor(&AirNode::Defplan(plan.clone()))
                        .map_err(|e| anyhow::anyhow!("hash defplan '{}': {e}", plan.name))?
                        .to_hex();
                    if let Some(existing) = seen.get(plan.name.as_str()) {
                        if existing != &hash {
                            anyhow::bail!(
                                "duplicate defplan '{}' has conflicting definitions ({}, {})",
                                plan.name,
                                existing,
                                hash
                            );
                        }
                        continue;
                    }
                    seen.insert(plan.name.clone(), hash.clone());
                    out.push((plan, hash));
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    Ok(out)
}

fn extract_contract(plan: &DefPlan, role: PlanRole, hash: String) -> PlanContract {
    let mut emitted_events = BTreeSet::new();
    for step in &plan.steps {
        if let PlanStepKind::RaiseEvent(raise) = &step.kind {
            emitted_events.insert(raise.event.as_str().to_string());
        }
    }

    let mut required_caps = plan.required_caps.clone();
    required_caps.sort();
    required_caps.dedup();

    let mut allowed_effects = plan
        .allowed_effects
        .iter()
        .map(|effect| effect.as_str().to_string())
        .collect::<Vec<_>>();
    allowed_effects.sort();
    allowed_effects.dedup();

    PlanContract {
        name: plan.name.clone(),
        role,
        input: plan.input.as_str().to_string(),
        output: plan
            .output
            .as_ref()
            .map(|schema| schema.as_str().to_string()),
        emitted_events: emitted_events.into_iter().collect(),
        required_caps,
        allowed_effects,
        hash,
    }
}

fn classify_role(plan_name: &str) -> PlanRole {
    let stem = plan_stem(plan_name);
    if stem.starts_with("entry_") {
        PlanRole::Entry
    } else if stem.starts_with("core_") {
        PlanRole::Core
    } else if stem.starts_with("adapter_") {
        PlanRole::Adapter
    } else if stem.starts_with("_internal_") {
        PlanRole::Internal
    } else {
        PlanRole::Unknown
    }
}

fn plan_stem(plan_name: &str) -> &str {
    let after_ns = plan_name.rsplit('/').next().unwrap_or(plan_name);
    after_ns.split('@').next().unwrap_or(after_ns)
}

fn flow_name_from_plan(plan_name: &str) -> String {
    let stem = plan_stem(plan_name);
    for prefix in ["entry_", "core_", "adapter_", "_internal_"] {
        if let Some(rest) = stem.strip_prefix(prefix) {
            return sanitize_id(rest);
        }
    }
    sanitize_id(stem)
}

fn infer_pack_name(import: &ResolvedAirImport) -> String {
    let root = &import.root;
    let root_name = root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| root.display().to_string());

    if root_name == "air" {
        if let Some(package) = import.expected_lock.package.as_ref() {
            return package.clone();
        }
        if let Some(path) = import.expected_lock.path.as_ref() {
            return sanitize_id(path);
        }
    }

    let parts = root
        .iter()
        .map(|segment| segment.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    for idx in 0..parts.len() {
        if parts[idx] == "plan-packs" && idx + 1 < parts.len() {
            return parts[idx + 1].clone();
        }
    }
    root_name
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

fn sanitize_namespace(value: &str) -> String {
    let id = sanitize_id(value);
    if id.is_empty() {
        "pack".to_string()
    } else {
        format!("pack.{}", id)
    }
}

fn to_pascal_case(value: &str) -> String {
    let mut out = String::new();
    for part in value
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    format!(
                        "{}{}",
                        first.to_ascii_uppercase(),
                        chars.as_str().to_ascii_lowercase()
                    )
                }
                None => String::new(),
            }
        })
    {
        out.push_str(&part);
    }
    if out.is_empty() {
        "Flow".to_string()
    } else {
        out
    }
}

fn asset_search_dirs(asset_root: &Path, include_root: bool) -> Result<Vec<PathBuf>> {
    if include_root {
        return Ok(vec![asset_root.to_path_buf()]);
    }

    let mut dirs: Vec<PathBuf> = Vec::new();

    if asset_root
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with("air") || n == "defs" || n == "plans")
        .unwrap_or(false)
    {
        dirs.push(asset_root.to_path_buf());
    }

    if asset_root.is_dir() {
        for entry in fs::read_dir(asset_root).context("read asset root")? {
            let entry = entry.context("read asset dir entry")?;
            if !entry.file_type().context("stat asset dir entry")?.is_dir() {
                continue;
            }
            let name_os = entry.file_name();
            let name = match name_os.to_str() {
                Some(s) => s.to_owned(),
                None => continue,
            };
            if name == "defs" || name == "plans" || name.starts_with("air") {
                dirs.push(entry.path());
            }
        }
    }

    dirs.sort();
    Ok(dirs)
}

fn collect_json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(dir) {
        let entry = entry.context("walk assets directory")?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        if matches!(path.extension().and_then(|s| s.to_str()), Some(ext) if ext.eq_ignore_ascii_case("json"))
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn parse_air_nodes(path: &Path) -> Result<Vec<AirNode>> {
    let data = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let trimmed = data.trim_start();
    if trimmed.starts_with('[') {
        let mut value: Value = serde_json::from_str(&data).context("parse AIR node array")?;
        normalize_authoring_hashes(&mut value);
        serde_json::from_value(value).context("deserialize AIR node array")
    } else if trimmed.is_empty() {
        Ok(Vec::new())
    } else {
        let mut value: Value = serde_json::from_str(&data).context("parse AIR node")?;
        normalize_authoring_hashes(&mut value);
        let node: AirNode = serde_json::from_value(value).context("deserialize AIR node")?;
        Ok(vec![node])
    }
}

fn normalize_authoring_hashes(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                normalize_authoring_hashes(item);
            }
        }
        Value::Object(map) => {
            if let Some(Value::String(kind)) = map.get("$kind") {
                match kind.as_str() {
                    "manifest" => normalize_manifest_authoring(map),
                    "defmodule" => ensure_hash_field(map, "wasm_hash"),
                    _ => {}
                }
            }
            for entry in map.values_mut() {
                normalize_authoring_hashes(entry);
            }
        }
        _ => {}
    }
}

fn normalize_manifest_authoring(map: &mut serde_json::Map<String, Value>) {
    for key in [
        "schemas", "modules", "plans", "caps", "policies", "effects", "secrets",
    ] {
        if let Some(Value::Array(entries)) = map.get_mut(key) {
            for entry in entries {
                if let Value::Object(obj) = entry {
                    normalize_named_ref_authoring(obj);
                }
            }
        }
    }
}

fn normalize_named_ref_authoring(map: &mut serde_json::Map<String, Value>) {
    if !matches!(map.get("name"), Some(Value::String(_))) {
        return;
    }
    ensure_hash_field(map, "hash");
}

fn ensure_hash_field(map: &mut serde_json::Map<String, Value>, key: &str) {
    let mut needs_insert = false;
    match map.get_mut(key) {
        Some(Value::String(current)) => {
            let trimmed = current.trim();
            if trimmed.is_empty()
                || trimmed.eq_ignore_ascii_case("sha256")
                || trimmed.eq_ignore_ascii_case("sha256:")
            {
                *current = ZERO_HASH_SENTINEL.to_string();
            }
        }
        Some(value @ Value::Null) => {
            *value = Value::String(ZERO_HASH_SENTINEL.to_string());
        }
        Some(_) => {}
        None => needs_insert = true,
    }

    if needs_insert {
        map.insert(
            key.to_string(),
            Value::String(ZERO_HASH_SENTINEL.to_string()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::{
        ExprOp, ExprOpCode, PlanBind, PlanBindEffect, PlanStep, PlanStepAwaitReceipt,
    };
    use serde_json::json;

    #[test]
    fn classify_role_by_stem_prefix() {
        assert_eq!(classify_role("aos.agent/entry_session@1"), PlanRole::Entry);
        assert_eq!(classify_role("aos.agent/core_session@1"), PlanRole::Core);
        assert_eq!(
            classify_role("aos.agent/adapter_session@1"),
            PlanRole::Adapter
        );
        assert_eq!(
            classify_role("aos.agent/_internal_session@1"),
            PlanRole::Internal
        );
        assert_eq!(classify_role("aos.agent/session@1"), PlanRole::Unknown);
    }

    #[test]
    fn flow_name_strips_role_prefix() {
        assert_eq!(
            flow_name_from_plan("aos.agent/core_workspace_sync@1"),
            "workspace_sync"
        );
        assert_eq!(
            flow_name_from_plan("aos.agent/entry_do_thing@1"),
            "do_thing"
        );
    }

    #[test]
    fn infer_pack_name_prefers_plan_packs_segment() {
        let import = ResolvedAirImport {
            root: PathBuf::from("/repo/crates/aos-agent-sdk/air/plan-packs/session-core"),
            expected_lock: crate::commands::sync::ImportLockPayload {
                source: "cargo".to_string(),
                package: Some("aos-agent-sdk".to_string()),
                version: Some("0.1.0".to_string()),
                source_id: None,
                manifest_path: Some("Cargo.toml".to_string()),
                air_dir: Some("air/plan-packs/session-core".to_string()),
                path: None,
                defs_hash: "sha256:abc".to_string(),
            },
        };
        assert_eq!(infer_pack_name(&import), "session-core");
    }

    #[test]
    fn infer_pack_name_prefers_package_for_root_air() {
        let import = ResolvedAirImport {
            root: PathBuf::from("/repo/crates/aos-agent-sdk/air"),
            expected_lock: crate::commands::sync::ImportLockPayload {
                source: "cargo".to_string(),
                package: Some("aos-agent-sdk".to_string()),
                version: Some("0.1.0".to_string()),
                source_id: None,
                manifest_path: Some("Cargo.toml".to_string()),
                air_dir: Some("air".to_string()),
                path: None,
                defs_hash: "sha256:def".to_string(),
            },
        };
        assert_eq!(infer_pack_name(&import), "aos-agent-sdk");
    }

    #[test]
    fn await_event_correlation_accepts_var_or_input_source() {
        let expr = Expr::Op(ExprOp {
            op: ExprOpCode::Eq,
            args: vec![
                Expr::Ref(aos_air_types::ExprRef {
                    reference: "@event.request_id".into(),
                }),
                Expr::Ref(aos_air_types::ExprRef {
                    reference: "@var:correlation_id".into(),
                }),
            ],
        });
        assert!(await_event_uses_correlation(Some(&expr), "request_id"));

        let plan_input = Expr::Op(ExprOp {
            op: ExprOpCode::Eq,
            args: vec![
                Expr::Ref(aos_air_types::ExprRef {
                    reference: "@event.request_id".into(),
                }),
                Expr::Ref(aos_air_types::ExprRef {
                    reference: "@plan.input.request_id".into(),
                }),
            ],
        });
        assert!(await_event_uses_correlation(
            Some(&plan_input),
            "request_id"
        ));

        let wrong_field = Expr::Op(ExprOp {
            op: ExprOpCode::Eq,
            args: vec![
                Expr::Ref(aos_air_types::ExprRef {
                    reference: "@event.request_id".into(),
                }),
                Expr::Ref(aos_air_types::ExprRef {
                    reference: "@plan.input.other_id".into(),
                }),
            ],
        });
        assert!(!await_event_uses_correlation(
            Some(&wrong_field),
            "request_id"
        ));
    }

    #[test]
    fn read_only_emit_effect_detection_handles_http_get_and_blob_get() {
        let http_get = PlanStepEmitEffect {
            kind: EffectKind::http_request(),
            params: ExprOrValue::Json(json!({ "record": { "method": { "text": "GET" } } })),
            cap: "cap_http".into(),
            idempotency_key: None,
            bind: PlanBindEffect {
                effect_id_as: "req".into(),
            },
        };
        assert!(emit_effect_is_read_only(&http_get));

        let http_post = PlanStepEmitEffect {
            kind: EffectKind::http_request(),
            params: ExprOrValue::Json(json!({ "record": { "method": { "text": "POST" } } })),
            cap: "cap_http".into(),
            idempotency_key: None,
            bind: PlanBindEffect {
                effect_id_as: "req".into(),
            },
        };
        assert!(!emit_effect_is_read_only(&http_post));

        let blob_get = PlanStepEmitEffect {
            kind: EffectKind::blob_get(),
            params: ExprOrValue::Json(json!({})),
            cap: "cap_blob".into(),
            idempotency_key: None,
            bind: PlanBindEffect {
                effect_id_as: "req".into(),
            },
        };
        assert!(emit_effect_is_read_only(&blob_get));
    }

    #[test]
    fn long_wait_plan_without_timeout_signal_is_detected() {
        let plan = DefPlan {
            name: "demo/core_waits@1".into(),
            input: "demo/Input@1".parse().unwrap(),
            output: None,
            locals: Default::default(),
            steps: vec![
                PlanStep {
                    id: "await_one".into(),
                    kind: PlanStepKind::AwaitReceipt(PlanStepAwaitReceipt {
                        for_expr: Expr::Ref(aos_air_types::ExprRef {
                            reference: "@var:req1".into(),
                        }),
                        bind: PlanBind { var: "r1".into() },
                    }),
                },
                PlanStep {
                    id: "await_two".into(),
                    kind: PlanStepKind::AwaitReceipt(PlanStepAwaitReceipt {
                        for_expr: Expr::Ref(aos_air_types::ExprRef {
                            reference: "@var:req2".into(),
                        }),
                        bind: PlanBind { var: "r2".into() },
                    }),
                },
            ],
            edges: vec![],
            required_caps: vec![],
            allowed_effects: vec![],
            invariants: vec![],
        };

        assert_eq!(plan_wait_count(&plan), 2);
        assert!(!plan_has_timeout_branch(&plan));
    }

    #[test]
    fn timer_set_marks_timeout_branch_present() {
        let plan = DefPlan {
            name: "demo/core_wait_timeout@1".into(),
            input: "demo/Input@1".parse().unwrap(),
            output: None,
            locals: Default::default(),
            steps: vec![PlanStep {
                id: "set_timeout".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::timer_set(),
                    params: ExprOrValue::Json(json!({})),
                    cap: "cap_timer".into(),
                    idempotency_key: None,
                    bind: PlanBindEffect {
                        effect_id_as: "t".into(),
                    },
                }),
            }],
            edges: vec![],
            required_caps: vec![],
            allowed_effects: vec![EffectKind::timer_set()],
            invariants: vec![],
        };

        assert!(plan_has_timeout_branch(&plan));
    }
}
