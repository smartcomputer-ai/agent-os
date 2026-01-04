//! `aos import` command.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::prelude::*;
use clap::{Args, ValueEnum};
use serde_json::json;

use aos_air_types::AirNode;
use aos_host::control::ControlClient;
use aos_host::host::WorldHost;
use aos_host::world_io::{
    BundleFilter, ImportMode, ImportOutcome, build_patch_document, import_bundle, load_air_bundle,
    manifest_node_hash, resolve_base_manifest, write_air_layout_with_options, WriteOptions,
};
use aos_kernel::patch_doc::{PatchDocument, compile_patch_document};
use aos_store::{FsStore, Store};

use crate::opts::{Mode, WorldOpts, resolve_dirs};
use crate::output::print_success;
use crate::util::validate_patch_json;

use super::{should_use_control, try_control_client};
use super::gov::autofill_patchdoc_hashes;
use super::gov::send_req as send_gov_req;

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportModeArg {
    Genesis,
    Patch,
}

#[derive(Args, Debug)]
pub struct ImportArgs {
    /// Import AIR assets from a directory (default: <world>/air when no inputs provided)
    #[arg(long)]
    pub air: Option<PathBuf>,

    /// Import mode for AIR (genesis or patch)
    #[arg(long = "import-mode", value_enum, default_value_t = ImportModeArg::Patch)]
    pub import_mode: ImportModeArg,

    /// AIR-only import (ignore modules/sources)
    #[arg(long)]
    pub air_only: bool,

    /// Dry-run: emit patch doc or manifest hash and exit
    #[arg(long)]
    pub dry_run: bool,

    /// Dev mode: auto-apply patches (skip governance unless explicitly requested; env: AOS_DEV)
    #[arg(long)]
    pub dev: bool,

    /// Propose the patch via governance control
    #[arg(long)]
    pub propose: bool,

    /// Run shadow evaluation after proposing
    #[arg(long)]
    pub shadow: bool,

    /// Approve after proposing (uses --approver)
    #[arg(long)]
    pub approve: bool,

    /// Apply after approval
    #[arg(long)]
    pub apply: bool,

    /// Optional description for proposal
    #[arg(long)]
    pub description: Option<String>,

    /// Optional base manifest hash override (patch mode)
    #[arg(long)]
    pub base: Option<String>,

    /// Require hashes in patch doc (no auto-fill)
    #[arg(long, default_value_t = false)]
    pub require_hashes: bool,

    /// Approver identity for --approve
    #[arg(long, default_value = "cli")]
    pub approver: String,
}

struct ImportResult {
    data: serde_json::Value,
    warnings: Vec<String>,
}

pub async fn cmd_import(opts: &WorldOpts, args: &ImportArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let mut warnings = Vec::new();
    let air_dir = args.air.clone().unwrap_or_else(|| dirs.air_dir.clone());
    let gov_mode = args.propose || args.approve || args.apply;
    let dev_mode = resolve_dev_mode(args) && !gov_mode;
    let res = import_air(opts, args, &dirs, &air_dir, dev_mode).await?;
    warnings.extend(res.warnings);
    let data = res.data;
    print_success(opts, data, None, warnings)
}

async fn import_air(
    opts: &WorldOpts,
    args: &ImportArgs,
    dirs: &crate::opts::ResolvedDirs,
    air_dir: &Path,
    dev_mode: bool,
) -> Result<ImportResult> {
    let store = FsStore::open(&dirs.store_root).context("open store")?;
    let filter = if args.air_only {
        BundleFilter::AirOnly
    } else {
        BundleFilter::Full
    };
    let bundle = load_air_bundle(std::sync::Arc::new(store.clone()), air_dir, filter)?;

    if args.air_only && args.import_mode != ImportModeArg::Patch {
        bail!("--air-only is only valid with --import-mode patch");
    }

    if args.import_mode == ImportModeArg::Genesis {
        if args.propose || args.shadow || args.approve || args.apply {
            bail!("--propose/--shadow/--approve/--apply are only valid in patch mode");
        }
        if args.dry_run {
            let hash = manifest_node_hash(&bundle.manifest)?;
            return Ok(ImportResult {
                data: json!({ "manifest_hash": hash }),
                warnings: Vec::new(),
            });
        }
        ensure_world_layout(&dirs)?;
        let outcome = import_bundle(&store, &bundle, ImportMode::Genesis)?;
        let ImportOutcome::Genesis(genesis) = outcome else {
            bail!("unexpected import outcome for genesis");
        };
        write_air_layout_with_options(
            &bundle,
            &genesis.manifest_bytes,
            &dirs.world,
            WriteOptions {
                include_sys: false,
                defs_bundle: false,
            },
        )?;
        return Ok(ImportResult {
            data: json!({ "manifest_hash": genesis.manifest_hash }),
            warnings: Vec::new(),
        });
    }

    let mut control = if should_use_control(opts) {
        try_control_client(&dirs).await
    } else {
        None
    };
    let manifest_path = dirs.store_root.join(".aos/manifest.air.cbor");
    let base = resolve_base_manifest(&store, args.base.clone(), control.as_mut(), &manifest_path)
        .await?;
    let doc = build_patch_document(&bundle, &base.manifest, &base.hash)?;
    let mut doc_json = serde_json::to_value(&doc).context("serialize patch doc")?;
    autofill_patchdoc_hashes(&mut doc_json, args.require_hashes)?;
    validate_patch_json(&doc_json)?;
    if args.dry_run {
        return Ok(ImportResult {
            data: doc_json,
            warnings: Vec::new(),
        });
    }

    if dev_mode && !args.shadow {
        let patch_bytes = serde_json::to_vec(&doc_json).context("encode patch JSON")?;
        if should_use_control(opts) {
            if let Some(mut client) = control.take() {
                let resp = send_gov_req(
                    &mut client,
                    "gov-apply-direct",
                    json!({ "patch_b64": BASE64_STANDARD.encode(patch_bytes) }),
                )
                .await?;
                let manifest_hash = resp
                    .result
                    .as_ref()
                    .and_then(|v| v.get("manifest_hash"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("missing manifest_hash in response"))?;
                return Ok(ImportResult {
                    data: json!({ "manifest_hash": manifest_hash }),
                    warnings: Vec::new(),
                });
            } else if matches!(opts.mode, Mode::Daemon) {
                bail!(
                    "daemon mode requested but no control socket at {}",
                    dirs.control_socket.display()
                );
            }
        }

        store.put_node(&AirNode::Manifest(base.manifest.clone()))?;
        let doc: PatchDocument =
            serde_json::from_value(doc_json.clone()).context("decode patch doc")?;
        let compiled = compile_patch_document(&store, doc).context("compile patch doc")?;
        let manifest_path = dirs.store_root.join(".aos/manifest.air.cbor");
        let host_config = crate::util::host_config_from_opts(
            opts.http_timeout_ms,
            opts.http_max_body_bytes,
        );
        let kernel_config = crate::util::make_kernel_config(&dirs.store_root)?;
        let mut host = WorldHost::open(
            std::sync::Arc::new(store.clone()),
            &manifest_path,
            host_config,
            kernel_config,
        )?;
        let manifest_hash = host.kernel_mut().apply_patch_direct(compiled)?;
        return Ok(ImportResult {
            data: json!({ "manifest_hash": manifest_hash }),
            warnings: Vec::new(),
        });
    }

    if dev_mode && args.shadow {
        let patch_bytes = serde_json::to_vec(&doc_json).context("encode patch JSON")?;
        if should_use_control(opts) {
            if let Some(mut client) = control.take() {
                let resp = send_gov_req(
                    &mut client,
                    "gov-propose",
                    json!({
                        "patch_b64": BASE64_STANDARD.encode(patch_bytes),
                        "description": args.description
                    }),
                )
                .await?;
                let proposal_id = resp
                    .result
                    .as_ref()
                    .and_then(|v| v.get("proposal_id"))
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow::anyhow!("missing proposal_id in response"))?;

                let mut extra = serde_json::Map::new();
                let resp = send_gov_req(
                    &mut client,
                    "gov-shadow",
                    json!({ "proposal_id": proposal_id }),
                )
                .await?;
                extra.insert(
                    "shadow".into(),
                    resp.result.unwrap_or_else(|| json!({})),
                );
                let resp = send_gov_req(
                    &mut client,
                    "gov-approve",
                    json!({
                        "proposal_id": proposal_id,
                        "decision": "approve",
                        "approver": args.approver
                    }),
                )
                .await?;
                extra.insert("approve".into(), json!({ "ok": resp.ok }));
                let resp = send_gov_req(
                    &mut client,
                    "gov-apply",
                    json!({ "proposal_id": proposal_id }),
                )
                .await?;
                extra.insert("apply".into(), json!({ "ok": resp.ok }));

                let mut data = serde_json::Map::new();
                data.insert("proposal_id".into(), json!(proposal_id));
                for (k, v) in extra {
                    data.insert(k, v);
                }
                return Ok(ImportResult {
                    data: serde_json::Value::Object(data),
                    warnings: Vec::new(),
                });
            } else if matches!(opts.mode, Mode::Daemon) {
                bail!(
                    "daemon mode requested but no control socket at {}",
                    dirs.control_socket.display()
                );
            }
        }

        store.put_node(&AirNode::Manifest(base.manifest.clone()))?;
        let doc: PatchDocument =
            serde_json::from_value(doc_json.clone()).context("decode patch doc")?;
        let compiled = compile_patch_document(&store, doc).context("compile patch doc")?;
        let manifest_path = dirs.store_root.join(".aos/manifest.air.cbor");
        let host_config = crate::util::host_config_from_opts(
            opts.http_timeout_ms,
            opts.http_max_body_bytes,
        );
        let kernel_config = crate::util::make_kernel_config(&dirs.store_root)?;
        let mut host = WorldHost::open(
            std::sync::Arc::new(store.clone()),
            &manifest_path,
            host_config,
            kernel_config,
        )?;
        let proposal_id = host
            .kernel_mut()
            .submit_proposal(compiled, args.description.clone())?;
        let summary = host.kernel_mut().run_shadow(proposal_id, None)?;
        host.kernel_mut()
            .approve_proposal(proposal_id, args.approver.clone())?;
        host.kernel_mut().apply_proposal(proposal_id)?;

        let mut data = serde_json::Map::new();
        data.insert("proposal_id".into(), json!(proposal_id));
        data.insert("shadow".into(), serde_json::to_value(summary)?);
        data.insert("approve".into(), json!({ "ok": true }));
        data.insert("apply".into(), json!({ "ok": true }));
        return Ok(ImportResult {
            data: serde_json::Value::Object(data),
            warnings: Vec::new(),
        });
    }

    if !args.propose {
        return Ok(ImportResult {
            data: doc_json,
            warnings: Vec::new(),
        });
    }

    let patch_bytes = serde_json::to_vec(&doc_json).context("encode patch JSON")?;
    let mut client = if let Some(client) = control.take() {
        client
    } else {
        ControlClient::connect(&dirs.control_socket)
            .await
            .context("connect control socket")?
    };
    let resp = send_gov_req(
        &mut client,
        "gov-propose",
        json!({
            "patch_b64": BASE64_STANDARD.encode(patch_bytes),
            "description": args.description
        }),
    )
    .await?;
    let proposal_id = resp
        .result
        .as_ref()
        .and_then(|v| v.get("proposal_id"))
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow::anyhow!("missing proposal_id in response"))?;

    if args.apply && !args.approve {
        bail!("--apply requires --approve");
    }

    let mut extra = serde_json::Map::new();
    if args.shadow {
        let resp = send_gov_req(
            &mut client,
            "gov-shadow",
            json!({ "proposal_id": proposal_id }),
        )
        .await?;
        extra.insert(
            "shadow".into(),
            resp.result.unwrap_or_else(|| json!({})),
        );
    }
    if args.approve {
        let resp = send_gov_req(
            &mut client,
            "gov-approve",
            json!({
                "proposal_id": proposal_id,
                "decision": "approve",
                "approver": args.approver
            }),
        )
        .await?;
        extra.insert("approve".into(), json!({ "ok": resp.ok }));
    }
    if args.apply {
        let resp = send_gov_req(
            &mut client,
            "gov-apply",
            json!({ "proposal_id": proposal_id }),
        )
        .await?;
        extra.insert("apply".into(), json!({ "ok": resp.ok }));
    }

    let mut data = serde_json::Map::new();
    data.insert("proposal_id".into(), json!(proposal_id));
    for (k, v) in extra {
        data.insert(k, v);
    }
    Ok(ImportResult {
        data: serde_json::Value::Object(data),
        warnings: Vec::new(),
    })
}

fn resolve_dev_mode(args: &ImportArgs) -> bool {
    if args.dev {
        return true;
    }
    env_truthy("AOS_DEV")
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|val| {
            matches!(
                val.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn ensure_world_layout(dirs: &crate::opts::ResolvedDirs) -> Result<()> {
    fs::create_dir_all(&dirs.world)?;
    fs::create_dir_all(dirs.world.join(".aos"))?;
    fs::create_dir_all(dirs.world.join("air"))?;
    fs::create_dir_all(dirs.world.join("modules"))?;
    fs::create_dir_all(dirs.world.join("reducer/src"))?;
    Ok(())
}
