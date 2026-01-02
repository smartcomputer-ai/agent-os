//! `aos import` command.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::prelude::*;
use clap::{Args, ValueEnum};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde_json::json;
use walkdir::WalkDir;

use aos_cbor::Hash;
use aos_host::control::ControlClient;
use aos_host::modes::batch::BatchRunner;
use aos_host::world_io::{
    BundleFilter, ImportMode, ImportOutcome, build_patch_document, import_bundle, load_air_bundle,
    manifest_node_hash, resolve_base_manifest, write_air_layout_with_options, WriteOptions,
};
use aos_store::{FsStore, Store};
use aos_sys::{ObjectMeta, ObjectRegistered};

use crate::key::{KeyOverrides, derive_event_key};
use crate::opts::{Mode, WorldOpts, resolve_dirs};
use crate::output::print_success;
use crate::util::{load_world_env, validate_patch_json};

use super::{create_host, prepare_world, should_use_control, try_control_client};
use super::gov::autofill_patchdoc_hashes;
use super::gov::send_req as send_gov_req;

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportModeArg {
    Genesis,
    Patch,
}

#[derive(Args, Debug)]
pub struct ImportArgs {
    /// Import AIR assets from a directory
    #[arg(long, conflicts_with = "source")]
    pub air: Option<PathBuf>,

    /// Import source bundle from a directory or tar file
    #[arg(long, conflicts_with = "air")]
    pub source: Option<PathBuf>,

    /// Import mode for AIR (genesis or patch)
    #[arg(long = "import-mode", value_enum, default_value_t = ImportModeArg::Patch)]
    pub import_mode: ImportModeArg,

    /// AIR-only import (ignore modules/sources)
    #[arg(long)]
    pub air_only: bool,

    /// Dry-run: emit patch doc or manifest hash and exit
    #[arg(long)]
    pub dry_run: bool,

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

    /// Object name for --source
    #[arg(long)]
    pub name: Option<String>,

    /// Tag for --source (repeatable)
    #[arg(long)]
    pub tag: Vec<String>,

    /// Owner for --source (default: cli)
    #[arg(long, default_value = "cli")]
    pub owner: String,
}

pub async fn cmd_import(opts: &WorldOpts, args: &ImportArgs) -> Result<()> {
    match (&args.air, &args.source) {
        (Some(_), None) => import_air(opts, args).await,
        (None, Some(_)) => import_source(opts, args).await,
        _ => bail!("--air or --source is required"),
    }
}

async fn import_air(opts: &WorldOpts, args: &ImportArgs) -> Result<()> {
    let air_dir = args.air.as_ref().expect("air");
    let dirs = resolve_dirs(opts)?;
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
            return print_success(opts, json!({ "manifest_hash": hash }), None, vec![]);
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
            WriteOptions { include_sys: false },
        )?;
        return print_success(
            opts,
            json!({ "manifest_hash": genesis.manifest_hash }),
            None,
            vec![],
        );
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

    if args.dry_run || !args.propose {
        return print_success(opts, doc_json, None, vec![]);
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
    print_success(opts, serde_json::Value::Object(data), None, vec![])
}

async fn import_source(opts: &WorldOpts, args: &ImportArgs) -> Result<()> {
    let source_path = args.source.as_ref().expect("source");
    let name = args
        .name
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("--name is required with --source"))?
        .to_string();
    if args.import_mode != ImportModeArg::Patch {
        bail!("--import-mode is only valid with --air");
    }
    if args.propose || args.shadow || args.approve || args.apply {
        bail!("--propose/--shadow/--approve/--apply are only valid with --air");
    }
    if args.air_only {
        bail!("--air-only is only valid with --air");
    }

    let dirs = resolve_dirs(opts)?;
    let mut warnings = Vec::new();
    let bundle_bytes = if source_path.is_dir() {
        build_source_bundle(source_path)?
    } else {
        fs::read(source_path).with_context(|| format!("read {}", source_path.display()))?
    };
    let hash = Hash::of_bytes(&bundle_bytes).to_hex();

    if args.dry_run {
        return print_success(
            opts,
            json!({ "hash": hash, "bytes": bundle_bytes.len() }),
            None,
            vec![],
        );
    }

    let stored_hash = store_blob(opts, &dirs, &bundle_bytes).await?;
    if stored_hash != hash {
        warnings.push("stored blob hash differs from computed hash".into());
    }

    let meta = ObjectMeta {
        name: name.clone(),
        kind: "source.bundle".into(),
        hash: stored_hash.clone(),
        tags: args.tag.iter().cloned().collect::<BTreeSet<_>>(),
        created_at: 0,
        owner: args.owner.clone(),
    };
    let event = ObjectRegistered { meta };
    let event_cbor = serde_cbor::to_vec(&event).context("encode ObjectRegistered")?;
    let event_json = serde_json::to_value(&event).context("encode ObjectRegistered json")?;
    let key = derive_event_key(&dirs, "sys/ObjectRegistered@1", &event_json, &KeyOverrides::default())?;

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let resp = client
                .send_event(
                    "cli-import-source",
                    "sys/ObjectRegistered@1",
                    key.as_deref(),
                    &event_cbor,
                )
                .await?;
            if !resp.ok {
                bail!("event-send failed: {:?}", resp.error);
            }
            return print_success(
                opts,
                json!({ "hash": stored_hash, "name": name }),
                None,
                warnings,
            );
        } else if matches!(opts.mode, Mode::Daemon) {
            bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        } else if !opts.quiet {
            warnings.push("daemon unavailable; using batch mode".into());
        }
    }

    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let host = create_host(store, loaded, &dirs, opts)?;
    let mut runner = BatchRunner::new(host);
    let events = vec![aos_host::host::ExternalEvent::DomainEvent {
        schema: "sys/ObjectRegistered@1".into(),
        value: event_cbor,
        key,
    }];
    let res = runner.step(events).await?;
    warnings.push(format!(
        "batch mode: effects={} receipts={}",
        res.cycle.effects_dispatched, res.cycle.receipts_applied
    ));
    print_success(
        opts,
        json!({ "hash": stored_hash, "name": name }),
        None,
        warnings,
    )
}

async fn store_blob(opts: &WorldOpts, dirs: &crate::opts::ResolvedDirs, bytes: &[u8]) -> Result<String> {
    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(dirs).await {
            let resp = client.put_blob("cli-import-source", bytes).await?;
            if !resp.ok {
                bail!("blob-put failed: {:?}", resp.error);
            }
            let hash = resp
                .result
                .as_ref()
                .and_then(|v| v.get("hash"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("blob-put missing hash"))?;
            return Ok(hash.to_string());
        }
    }

    let store = FsStore::open(&dirs.store_root).context("open store")?;
    let stored = store.put_blob(bytes).context("store blob")?;
    Ok(stored.to_hex())
}

fn ensure_world_layout(dirs: &crate::opts::ResolvedDirs) -> Result<()> {
    fs::create_dir_all(&dirs.world)?;
    fs::create_dir_all(dirs.world.join(".aos"))?;
    fs::create_dir_all(dirs.world.join("air"))?;
    fs::create_dir_all(dirs.world.join("modules"))?;
    fs::create_dir_all(dirs.world.join("reducer/src"))?;
    Ok(())
}

fn build_source_bundle(root: &Path) -> Result<Vec<u8>> {
    let matcher = IgnoreMatcher::new(root)?;
    let mut entries = Vec::new();
    let mut iter = WalkDir::new(root).follow_links(false).into_iter();
    while let Some(entry) = iter.next() {
        let entry = entry.context("walk source dir")?;
        let path = entry.path();
        if path == root {
            continue;
        }
        let rel = path.strip_prefix(root).context("strip source prefix")?;
        if matcher.is_ignored(rel, entry.file_type().is_dir()) {
            if entry.file_type().is_dir() {
                iter.skip_current_dir();
            }
            continue;
        }
        if entry.file_type().is_symlink() {
            continue;
        }
        let rel_str = normalize_rel_path(rel);
        let metadata = entry.metadata().context("stat source entry")?;
        let mode = normalized_mode(&metadata, entry.file_type().is_dir());
        let size = if entry.file_type().is_dir() { 0 } else { metadata.len() };
        entries.push(SourceEntry {
            rel_path: rel_str,
            fs_path: path.to_path_buf(),
            is_dir: entry.file_type().is_dir(),
            mode,
            size,
        });
    }

    entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    let mut builder = tar::Builder::new(Vec::new());
    for entry in entries {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(if entry.is_dir {
            tar::EntryType::Directory
        } else {
            tar::EntryType::Regular
        });
        header.set_path(&entry.rel_path)?;
        header.set_mode(entry.mode);
        header.set_uid(0);
        header.set_gid(0);
        header.set_username("")?;
        header.set_groupname("")?;
        header.set_mtime(0);
        header.set_size(entry.size);
        header.set_cksum();

        if entry.is_dir {
            builder.append(&header, std::io::empty())?;
        } else {
            let mut file = fs::File::open(&entry.fs_path)
                .with_context(|| format!("read {}", entry.fs_path.display()))?;
            builder.append(&header, &mut file)?;
        }
    }
    builder.into_inner().context("finalize tar")
}

fn normalize_rel_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    raw.replace('\\', "/")
}

fn normalized_mode(metadata: &fs::Metadata, is_dir: bool) -> u32 {
    if is_dir {
        return 0o755;
    }
    let exec = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode();
            mode & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            false
        }
    };
    if exec {
        0o755
    } else {
        0o644
    }
}

struct SourceEntry {
    rel_path: String,
    fs_path: PathBuf,
    is_dir: bool,
    mode: u32,
    size: u64,
}

struct IgnoreMatcher {
    gitignore: Option<Gitignore>,
    aosignore: Option<Gitignore>,
}

impl IgnoreMatcher {
    fn new(root: &Path) -> Result<Self> {
        let gitignore = build_ignore(root, ".gitignore")?;
        let aosignore = build_ignore(root, ".aosignore")?;
        Ok(Self {
            gitignore,
            aosignore,
        })
    }

    fn is_ignored(&self, rel: &Path, is_dir: bool) -> bool {
        if is_implicit_ignore(rel) {
            return true;
        }
        let mut ignored = false;
        if let Some(gitignore) = &self.gitignore {
            ignored = apply_ignore_match(gitignore.matched(rel, is_dir), ignored);
        }
        if let Some(aosignore) = &self.aosignore {
            ignored = apply_ignore_match(aosignore.matched(rel, is_dir), ignored);
        }
        ignored
    }
}

fn build_ignore(root: &Path, name: &str) -> Result<Option<Gitignore>> {
    let path = root.join(name);
    if !path.exists() {
        return Ok(None);
    }
    let mut builder = GitignoreBuilder::new(root);
    builder
        .add(path)
        .context("add ignore file to builder")?;
    let ignore = builder.build().context("build ignore matcher")?;
    Ok(Some(ignore))
}

fn apply_ignore_match<T>(matched: ignore::Match<T>, current: bool) -> bool {
    if matched.is_ignore() {
        return true;
    }
    if matched.is_whitelist() {
        return false;
    }
    current
}

fn is_implicit_ignore(rel: &Path) -> bool {
    let rel_str = normalize_rel_path(rel);
    rel_str == ".git"
        || rel_str.starts_with(".git/")
        || rel_str == ".aos"
        || rel_str.starts_with(".aos/")
}
