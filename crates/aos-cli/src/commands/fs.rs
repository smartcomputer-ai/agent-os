//! `aos world fs` command: filesystem-like introspection over world state and ObjectCatalog.
//!
//! Implements `ls`, `stat`, and `cat` with daemon-first / batch-fallback behavior.

use std::collections::BTreeSet;
use std::io::Write;
use std::str::FromStr;

use anyhow::{Context, Result, anyhow};
use aos_cbor::Hash;
use aos_host::control::ClientCellEntry;
use aos_kernel::cell_index::CellMeta;
use aos_kernel::query::ReadMeta;
use aos_kernel::{Consistency, StateReader};
use aos_store::Store;
use aos_sys::{ObjectMeta, ObjectVersions};
use base64::Engine;
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_cbor;
use serde_json::Value as JsonValue;

use crate::opts::ResolvedDirs;
use crate::opts::{WorldOpts, resolve_dirs};
use crate::util::load_world_env;

use super::{create_host, prepare_world, try_control_client};

const OBJ_REDUCER: &str = "sys/ObjectCatalog@1";

#[derive(Args, Debug)]
pub struct FsArgs {
    #[command(subcommand)]
    cmd: FsSubcommand,
}

#[derive(Subcommand, Debug)]
enum FsSubcommand {
    /// List entries
    Ls(LsArgs),
    /// Show metadata for a path
    Stat(StatArgs),
    /// Read content at a path
    Cat(CatArgs),
}

#[derive(Args, Debug)]
pub struct LsArgs {
    /// Path to list
    pub path: String,
    /// Show detailed columns (hash, size, version, kind)
    #[arg(long)]
    pub long: bool,
    /// Output JSON
    #[arg(long)]
    pub json: bool,
    /// Filter objects by kind (only for /obj)
    #[arg(long)]
    pub kind: Option<String>,
    /// Recurse into sub-paths (only for /obj)
    #[arg(long)]
    pub recursive: bool,
}

#[derive(Args, Debug)]
pub struct StatArgs {
    /// Path to stat
    pub path: String,
}

#[derive(Args, Debug)]
pub struct CatArgs {
    /// Path to read
    pub path: String,
    /// Treat output as raw bytes (no pretty JSON)
    #[arg(long)]
    pub raw: bool,
}

#[derive(Debug, Clone)]
enum FsPath {
    SysManifest,
    SysJournalHead,
    SysReducer { name: String, key: Option<KeyRef> },
    ObjMeta { name: String, version: Option<u64> },
    ObjData { name: String, version: Option<u64> },
    Blob { hash: String },
}

#[derive(Debug, Clone)]
enum KeyRef {
    Utf8(String),
    Hex(String),
}

#[derive(Debug, Serialize)]
struct Entry {
    path: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    object_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<u64>,
}

#[derive(Debug, Serialize)]
struct StatOut {
    path: String,
    exists: bool,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    object_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<BTreeSet<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    meta: Option<MetaOut>,
}

#[derive(Debug, Serialize, Clone)]
struct MetaOut {
    journal_height: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_hash: Option<String>,
    manifest_hash: String,
}

impl From<ReadMeta> for MetaOut {
    fn from(m: ReadMeta) -> Self {
        MetaOut {
            journal_height: m.journal_height,
            snapshot_hash: m.snapshot_hash.map(|h| h.to_hex()),
            manifest_hash: m.manifest_hash.to_hex(),
        }
    }
}

pub async fn cmd_fs(opts: &WorldOpts, args: &FsArgs) -> Result<()> {
    match &args.cmd {
        FsSubcommand::Ls(a) => cmd_ls(opts, a).await,
        FsSubcommand::Stat(a) => cmd_stat(opts, a).await,
        FsSubcommand::Cat(a) => cmd_cat(opts, a).await,
    }
}

// -----------------------------------------------------------------------------=
// LS
// -----------------------------------------------------------------------------=

async fn cmd_ls(opts: &WorldOpts, args: &LsArgs) -> Result<()> {
    let path = args.path.as_str();
    let parsed = parse_path(path)?;
    match parsed {
        FsPath::SysManifest | FsPath::SysJournalHead | FsPath::SysReducer { .. } => {
            // For sys paths, delegate to stat to show single entry.
            let stat_args = StatArgs {
                path: args.path.clone(),
            };
            return cmd_stat(opts, &stat_args).await;
        }
        FsPath::Blob { .. } => {
            let stat_args = StatArgs {
                path: args.path.clone(),
            };
            return cmd_stat(opts, &stat_args).await;
        }
        FsPath::ObjMeta { .. } | FsPath::ObjData { .. } => {}
    }

    let dirs = resolve_dirs(opts)?;

    // Collect entries
    let entries = if let Some(mut client) = try_control_client(&dirs).await {
        list_objects_control(&mut client, &parsed, args).await?
    } else {
        list_objects_batch(&dirs, opts, &parsed, args)?
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        for e in entries {
            if args.long {
                let hash = e.hash.unwrap_or_default();
                let size = e.size.unwrap_or(0);
                let ver = e.version.map(|v| format!("v{v}")).unwrap_or_default();
                let okind = e.object_kind.unwrap_or_default();
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    e.path,
                    e.kind,
                    okind,
                    ver,
                    if hash.is_empty() { "-" } else { &hash }
                );
                if size > 0 {
                    println!("  size: {size}");
                }
            } else {
                println!("{}", e.path);
            }
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------=
// STAT
// -----------------------------------------------------------------------------=

async fn cmd_stat(opts: &WorldOpts, args: &StatArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let parsed = parse_path(&args.path)?;

    if let Some(mut client) = try_control_client(&dirs).await {
        let out = stat_control(&mut client, &parsed).await?;
        print_stat(out)?;
    } else {
        load_world_env(&dirs.world)?;
        let (store, loaded) = prepare_world(&dirs, opts)?;
        let host = create_host(store, loaded, &dirs, opts)?;
        let out = stat_batch(&host, &parsed)?;
        print_stat(out)?;
    }
    Ok(())
}

fn print_stat(out: StatOut) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

// -----------------------------------------------------------------------------=
// CAT
// -----------------------------------------------------------------------------=

async fn cmd_cat(opts: &WorldOpts, args: &CatArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let parsed = parse_path(&args.path)?;

    match parsed {
        FsPath::SysManifest | FsPath::SysReducer { .. } | FsPath::SysJournalHead => {
            // fall through to normal handling
        }
        FsPath::Blob { .. } | FsPath::ObjData { .. } => {}
        FsPath::ObjMeta { .. } => {
            // cat on metadata should behave like stat
            let stat_args = StatArgs {
                path: args.path.clone(),
            };
            return cmd_stat(opts, &stat_args).await;
        }
    }

    let (meta_opt, bytes) = if let Some(mut client) = try_control_client(&dirs).await {
        read_control(&mut client, &parsed).await?
    } else {
        load_world_env(&dirs.world)?;
        let (store, loaded) = prepare_world(&dirs, opts)?;
        let host = create_host(store, loaded, &dirs, opts)?;
        read_batch(&host, &parsed)?
    };

    if let Some(meta) = meta_opt {
        eprintln!(
            "meta: {}",
            serde_json::to_string_pretty(&serde_json::json!({
                "journal_height": meta.journal_height,
                "snapshot_hash": meta.snapshot_hash,
                "manifest_hash": meta.manifest_hash,
            }))?
        );
    }

    // Try to decode JSON for reducer/manifest reads; otherwise raw bytes/hex.
    match serde_cbor::from_slice::<JsonValue>(&bytes) {
        Ok(json) if !args.raw => {
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
        _ => {
            // treat as bytes
            std::io::stdout().write_all(&bytes)?;
        }
    }
    Ok(())
}

// -----------------------------------------------------------------------------=
// Path parsing
// -----------------------------------------------------------------------------=

fn parse_path(input: &str) -> Result<FsPath> {
    if !input.starts_with('/') {
        anyhow::bail!("path must be absolute and start with /");
    }
    let parts: Vec<&str> = input
        .trim_end_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        anyhow::bail!("path must not be root");
    }
    match parts[0] {
        "sys" => {
            if parts.len() == 1 {
                anyhow::bail!("use /sys/manifest or /sys/reducers/...");
            }
            match parts[1] {
                "manifest" => Ok(FsPath::SysManifest),
                "journal" if parts.get(2) == Some(&"head") => Ok(FsPath::SysJournalHead),
                "reducers" => {
                    let name = parts
                        .get(2)
                        .ok_or_else(|| anyhow!("missing reducer name"))?
                        .to_string();
                    let key = if parts.len() > 3 {
                        let raw = parts[3];
                        if let Some(rest) = raw.strip_prefix("0x") {
                            KeyRef::Hex(rest.to_string())
                        } else {
                            KeyRef::Utf8(raw.to_string())
                        }
                    } else {
                        KeyRef::Utf8(String::new())
                    };
                    Ok(FsPath::SysReducer {
                        name,
                        key: if parts.len() > 3 { Some(key) } else { None },
                    })
                }
                _ => anyhow::bail!("unknown /sys path"),
            }
        }
        "obj" => {
            if parts.len() == 1 {
                return Ok(FsPath::ObjMeta {
                    name: String::new(),
                    version: None,
                });
            }
            let mut rest = &parts[1..];
            let mut version = None;
            let is_data = if let Some(last) = rest.last() {
                *last == "data"
            } else {
                false
            };
            if is_data {
                rest = &rest[..rest.len() - 1];
            }
            if let Some(last) = rest.last() {
                if let Some(v) = last.strip_prefix('v') {
                    if let Ok(n) = u64::from_str(v) {
                        version = Some(n);
                        rest = &rest[..rest.len() - 1];
                    }
                }
            }
            let name = rest.join("/");
            if is_data {
                Ok(FsPath::ObjData { name, version })
            } else {
                Ok(FsPath::ObjMeta { name, version })
            }
        }
        "blob" => {
            let hash = parts
                .get(1)
                .ok_or_else(|| anyhow!("missing hash"))?
                .to_string();
            Ok(FsPath::Blob { hash })
        }
        _ => anyhow::bail!("unknown path root"),
    }
}

// -----------------------------------------------------------------------------=
// Helpers: control path
// -----------------------------------------------------------------------------=

async fn list_objects_control(
    client: &mut aos_host::control::ControlClient,
    parsed: &FsPath,
    args: &LsArgs,
) -> Result<Vec<Entry>> {
    // We only list for /obj
    let (_, metas) = client.list_cells_decoded("cli-fs-ls", OBJ_REDUCER).await?;
    let names = decode_cell_names_control(metas);
    let mut entries = Vec::new();
    for name in names {
        if !matches_prefix(&name, parsed) {
            continue;
        }
        let (meta, ver, hash) = fetch_latest_object_control(client, &name).await?;
        if let Some(kind) = args.kind.as_ref() {
            if &meta.kind != kind {
                continue;
            }
        }
        let (target_hash, target_ver) = select_version(&meta, ver, hash, None);
        let obj_path = format!("/obj/{}", name);
        entries.push(Entry {
            path: if args.recursive {
                obj_path.clone()
            } else {
                obj_path
            },
            kind: "object".into(),
            object_kind: Some(meta.kind.clone()),
            hash: Some(target_hash.clone()),
            size: None,
            version: Some(target_ver),
        });
        if args.recursive {
            // emit data leaf
            entries.push(Entry {
                path: format!("/obj/{}/data", name),
                kind: "data".into(),
                object_kind: Some(meta.kind.clone()),
                hash: Some(target_hash.clone()),
                size: None,
                version: Some(target_ver),
            });
        }
    }
    Ok(entries)
}

async fn fetch_latest_object_control(
    client: &mut aos_host::control::ControlClient,
    name: &str,
) -> Result<(ObjectMeta, u64, String)> {
    let key_bytes = cbor_key_bytes(name)?;
    let (_meta, state_opt) = client
        .query_state_decoded("cli-fs-obj", OBJ_REDUCER, Some(&key_bytes), None)
        .await?;
    let state_bytes = state_opt.context("object state missing")?;
    let versions: ObjectVersions = serde_cbor::from_slice(&state_bytes)?;
    let latest = versions.latest;
    let meta = versions
        .versions
        .get(&latest)
        .cloned()
        .context("latest version missing")?;
    Ok((meta.clone(), latest, meta.hash.clone()))
}

async fn stat_control(
    client: &mut aos_host::control::ControlClient,
    parsed: &FsPath,
) -> Result<StatOut> {
    match parsed {
        FsPath::SysManifest => {
            let (meta, bytes) = client.manifest_read("cli-fs-manifest", None).await?;
            Ok(StatOut {
                path: "/sys/manifest".into(),
                exists: true,
                kind: "manifest".into(),
                object_kind: None,
                hash: Some(Hash::of_bytes(&bytes).to_hex()),
                size: Some(bytes.len() as u64),
                version: None,
                tags: None,
                created_at: None,
                meta: Some(meta.into()),
            })
        }
        FsPath::SysJournalHead => {
            let meta = client.journal_head_meta("cli-fs-head").await?;
            Ok(StatOut {
                path: "/sys/journal/head".into(),
                exists: true,
                kind: "journal_head".into(),
                object_kind: None,
                hash: None,
                size: None,
                version: None,
                tags: None,
                created_at: None,
                meta: Some(meta.into()),
            })
        }
        FsPath::SysReducer { name, key } => {
            let key_bytes = key_bytes_opt(key)?;
            let (meta, state) = client
                .query_state_decoded("cli-fs-state", name, key_bytes.as_deref(), None)
                .await?;
            let exists = state.is_some();
            let size = state.as_ref().map(|b| b.len() as u64);
            Ok(StatOut {
                path: format!(
                    "/sys/reducers/{}{}",
                    name,
                    key.as_ref()
                        .map(|k| format!("/{}", display_key(k)))
                        .unwrap_or_default()
                ),
                exists,
                kind: "reducer_state".into(),
                object_kind: None,
                hash: state.map(|b| Hash::of_bytes(&b).to_hex()),
                size,
                version: None,
                tags: None,
                created_at: None,
                meta: Some(meta.into()),
            })
        }
        FsPath::ObjMeta { name, version } => {
            let key_bytes = cbor_key_bytes(name)?;
            let (meta, state_opt) = client
                .query_state_decoded("cli-fs-obj", OBJ_REDUCER, Some(&key_bytes), None)
                .await?;
            let versions: ObjectVersions =
                serde_cbor::from_slice(&state_opt.context("object not found")?)?;
            let (meta_v, ver_num, hash) = select_version_explicit(&versions, *version)?;
            Ok(StatOut {
                path: build_obj_path(name, *version, false),
                exists: true,
                kind: "object".into(),
                object_kind: Some(meta_v.kind.clone()),
                hash: Some(hash),
                size: None,
                version: Some(ver_num),
                tags: Some(meta_v.tags.clone()),
                created_at: Some(meta_v.created_at),
                meta: Some(meta.into()),
            })
        }
        FsPath::ObjData { name, version } => {
            let key_bytes = cbor_key_bytes(name)?;
            let (meta, state_opt) = client
                .query_state_decoded("cli-fs-obj", OBJ_REDUCER, Some(&key_bytes), None)
                .await?;
            let versions: ObjectVersions =
                serde_cbor::from_slice(&state_opt.context("object not found")?)?;
            let (meta_v, ver_num, hash) = select_version_explicit(&versions, *version)?;
            let size = None;
            Ok(StatOut {
                path: build_obj_path(name, *version, true),
                exists: true,
                kind: "blob".into(),
                object_kind: Some(meta_v.kind.clone()),
                hash: Some(hash),
                size,
                version: Some(ver_num),
                tags: Some(meta_v.tags.clone()),
                created_at: Some(meta_v.created_at),
                meta: Some(meta.into()),
            })
        }
        FsPath::Blob { hash } => {
            let meta = client.journal_head_meta("cli-fs-blob-meta").await?;
            Ok(StatOut {
                path: format!("/blob/{}", hash),
                exists: true,
                kind: "blob".into(),
                object_kind: None,
                hash: Some(hash.to_string()),
                size: None,
                version: None,
                tags: None,
                created_at: None,
                meta: Some(meta.into()),
            })
        }
    }
}

async fn read_control(
    client: &mut aos_host::control::ControlClient,
    parsed: &FsPath,
) -> Result<(Option<MetaOut>, Vec<u8>)> {
    match parsed {
        FsPath::SysManifest => {
            let (meta, bytes) = client.manifest_read("cli-fs-manifest", None).await?;
            Ok((Some(meta.into()), bytes))
        }
        FsPath::SysJournalHead => {
            let meta = client.journal_head_meta("cli-fs-head").await?;
            let bytes = serde_cbor::to_vec(&serde_json::json!({
                "journal_height": meta.journal_height,
                "snapshot_hash": meta.snapshot_hash.as_ref().map(|h| h.to_hex()),
                "manifest_hash": meta.manifest_hash.to_hex(),
            }))?;
            Ok((Some(meta.into()), bytes))
        }
        FsPath::SysReducer { name, key } => {
            let key_bytes = key_bytes_opt(key)?;
            let (meta, state_opt) = client
                .query_state_decoded("cli-fs-state", name, key_bytes.as_deref(), None)
                .await?;
            let bytes = state_opt.unwrap_or_default();
            Ok((Some(meta.into()), bytes))
        }
        FsPath::ObjMeta { name, version } => {
            let key_bytes = cbor_key_bytes(name)?;
            let (_meta, state_opt) = client
                .query_state_decoded("cli-fs-obj", OBJ_REDUCER, Some(&key_bytes), None)
                .await?;
            let versions: ObjectVersions =
                serde_cbor::from_slice(&state_opt.context("object not found")?)?;
            let (meta_v, _, _) = select_version_explicit(&versions, *version)?;
            let bytes = serde_cbor::to_vec(&meta_v)?;
            Ok((None, bytes))
        }
        FsPath::ObjData { name, version } => {
            let key_bytes = cbor_key_bytes(name)?;
            let (_meta, state_opt) = client
                .query_state_decoded("cli-fs-obj", OBJ_REDUCER, Some(&key_bytes), None)
                .await?;
            let versions: ObjectVersions =
                serde_cbor::from_slice(&state_opt.context("object not found")?)?;
            let (_meta_v, _, hash) = select_version_explicit(&versions, *version)?;
            let head = client.journal_head_meta("cli-fs-blob-meta").await?;
            let data = client
                .blob_get("cli-fs-blob", hash.trim_start_matches("sha256:"))
                .await?;
            Ok((Some(head.into()), data))
        }
        FsPath::Blob { hash } => {
            let head = client.journal_head_meta("cli-fs-blob-meta").await?;
            let data = client.blob_get("cli-fs-blob", hash).await?;
            Ok((Some(head.into()), data))
        }
    }
}

// -----------------------------------------------------------------------------=
// Helpers: batch path
// -----------------------------------------------------------------------------=

fn list_objects_batch(
    dirs: &ResolvedDirs,
    opts: &WorldOpts,
    parsed: &FsPath,
    args: &LsArgs,
) -> Result<Vec<Entry>> {
    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(dirs, opts)?;
    let host = create_host(store, loaded, dirs, opts)?;
    let metas = host.list_cells(OBJ_REDUCER).context("list catalog cells")?;
    let names = decode_cell_names_batch(metas);
    let mut entries = Vec::new();
    for name in names {
        if !matches_prefix(&name, parsed) {
            continue;
        }
        let (meta_v, ver_num, hash) = fetch_latest_object_batch(&host, &name)?;
        if let Some(kind) = args.kind.as_ref() {
            if &meta_v.kind != kind {
                continue;
            }
        }
        entries.push(Entry {
            path: format!("/obj/{}", name),
            kind: "object".into(),
            object_kind: Some(meta_v.kind.clone()),
            hash: Some(hash),
            size: None,
            version: Some(ver_num),
        });
        if args.recursive {
            entries.push(Entry {
                path: format!("/obj/{}/data", name),
                kind: "data".into(),
                object_kind: Some(meta_v.kind.clone()),
                hash: Some(meta_v.hash.clone()),
                size: None,
                version: Some(ver_num),
            });
        }
    }
    Ok(entries)
}

fn fetch_latest_object_batch(
    host: &aos_host::host::WorldHost<aos_store::FsStore>,
    name: &str,
) -> Result<(ObjectMeta, u64, String)> {
    let key_bytes = cbor_key_bytes(name)?;
    let read = host
        .query_state(OBJ_REDUCER, Some(&key_bytes), Consistency::Head)
        .ok_or_else(|| anyhow!("no catalog state"))?;
    let versions: ObjectVersions = serde_cbor::from_slice(&read.value.context("missing value")?)?;
    let latest = versions.latest;
    let meta = versions
        .versions
        .get(&latest)
        .cloned()
        .context("latest version missing")?;
    Ok((meta.clone(), latest, meta.hash.clone()))
}

fn stat_batch(
    host: &aos_host::host::WorldHost<aos_store::FsStore>,
    parsed: &FsPath,
) -> Result<StatOut> {
    match parsed {
        FsPath::SysManifest => {
            let read = host.kernel().get_manifest(Consistency::Head)?;
            let bytes = aos_cbor::to_canonical_cbor(&read.value)?;
            Ok(StatOut {
                path: "/sys/manifest".into(),
                exists: true,
                kind: "manifest".into(),
                object_kind: None,
                hash: Some(Hash::of_bytes(&bytes).to_hex()),
                size: Some(bytes.len() as u64),
                version: None,
                tags: None,
                created_at: None,
                meta: Some(read.meta.into()),
            })
        }
        FsPath::SysJournalHead => {
            let meta = host.kernel().get_journal_head();
            Ok(StatOut {
                path: "/sys/journal/head".into(),
                exists: true,
                kind: "journal_head".into(),
                object_kind: None,
                hash: None,
                size: None,
                version: None,
                tags: None,
                created_at: None,
                meta: Some(meta.into()),
            })
        }
        FsPath::SysReducer { name, key } => {
            let key_bytes = key_bytes_opt(key)?;
            let read_opt = host.query_state(name, key_bytes.as_deref(), Consistency::Head);
            let (exists, size, hash, meta_opt) = if let Some(read) = read_opt {
                let exists = read.value.is_some();
                let size = read.value.as_ref().map(|b| b.len() as u64);
                let hash = read.value.as_ref().map(|b| Hash::of_bytes(b).to_hex());
                (exists, size, hash, Some(read.meta.into()))
            } else {
                (false, None, None, None)
            };
            Ok(StatOut {
                path: format!(
                    "/sys/reducers/{}{}",
                    name,
                    key.as_ref()
                        .map(|k| format!("/{}", display_key(k)))
                        .unwrap_or_default()
                ),
                exists,
                kind: "reducer_state".into(),
                object_kind: None,
                hash,
                size,
                version: None,
                tags: None,
                created_at: None,
                meta: meta_opt,
            })
        }
        FsPath::ObjMeta { name, version } => {
            let (meta_v, ver_num, hash, meta) = fetch_object_batch(host, name, *version)?;
            Ok(StatOut {
                path: build_obj_path(name, *version, false),
                exists: true,
                kind: "object".into(),
                object_kind: Some(meta_v.kind.clone()),
                hash: Some(hash),
                size: None,
                version: Some(ver_num),
                tags: Some(meta_v.tags.clone()),
                created_at: Some(meta_v.created_at),
                meta: Some(meta),
            })
        }
        FsPath::ObjData { name, version } => {
            let (meta_v, ver_num, hash, meta) = fetch_object_batch(host, name, *version)?;
            Ok(StatOut {
                path: build_obj_path(name, *version, true),
                exists: true,
                kind: "blob".into(),
                object_kind: Some(meta_v.kind.clone()),
                hash: Some(hash),
                size: None,
                version: Some(ver_num),
                tags: Some(meta_v.tags.clone()),
                created_at: Some(meta_v.created_at),
                meta: Some(meta),
            })
        }
        FsPath::Blob { hash } => {
            let meta = host.kernel().get_journal_head();
            Ok(StatOut {
                path: format!("/blob/{}", hash),
                exists: true,
                kind: "blob".into(),
                object_kind: None,
                hash: Some(hash.to_string()),
                size: None,
                version: None,
                tags: None,
                created_at: None,
                meta: Some(meta.into()),
            })
        }
    }
}

fn read_batch(
    host: &aos_host::host::WorldHost<aos_store::FsStore>,
    parsed: &FsPath,
) -> Result<(Option<MetaOut>, Vec<u8>)> {
    match parsed {
        FsPath::SysManifest => {
            let read = host.kernel().get_manifest(Consistency::Head)?;
            let bytes = aos_cbor::to_canonical_cbor(&read.value)?;
            Ok((Some(read.meta.into()), bytes))
        }
        FsPath::SysJournalHead => {
            let meta = host.kernel().get_journal_head();
            let bytes = serde_cbor::to_vec(&serde_json::json!({
                "journal_height": meta.journal_height,
                "snapshot_hash": meta.snapshot_hash.as_ref().map(|h| h.to_hex()),
                "manifest_hash": meta.manifest_hash.to_hex(),
            }))?;
            Ok((Some(meta.into()), bytes))
        }
        FsPath::SysReducer { name, key } => {
            let key_bytes = key_bytes_opt(key)?;
            let read = host
                .query_state(name, key_bytes.as_deref(), Consistency::Head)
                .ok_or_else(|| anyhow!("state not found"))?;
            let bytes = read.value.unwrap_or_default();
            Ok((Some(read.meta.into()), bytes))
        }
        FsPath::ObjMeta { name, version } => {
            let (meta_v, _, _, _) = fetch_object_batch(host, name, *version)?;
            let bytes = serde_cbor::to_vec(&meta_v)?;
            Ok((None, bytes))
        }
        FsPath::ObjData { name, version } => {
            let (_meta_v, _, hash, meta) = fetch_object_batch(host, name, *version)?;
            let hash_obj = Hash::from_hex_str(hash.trim_start_matches("sha256:"))
                .or_else(|_| Hash::from_hex_str(&hash))?;
            let data = host.store().get_blob(hash_obj)?;
            Ok((Some(meta), data))
        }
        FsPath::Blob { hash } => {
            let meta = host.kernel().get_journal_head();
            let hash_obj = Hash::from_hex_str(hash)?;
            let data = host.store().get_blob(hash_obj)?;
            Ok((Some(meta.into()), data))
        }
    }
}

fn fetch_object_batch(
    host: &aos_host::host::WorldHost<aos_store::FsStore>,
    name: &str,
    version: Option<u64>,
) -> Result<(ObjectMeta, u64, String, MetaOut)> {
    let key_bytes = cbor_key_bytes(name)?;
    let read = host
        .query_state(OBJ_REDUCER, Some(&key_bytes), Consistency::Head)
        .ok_or_else(|| anyhow!("object missing"))?;
    let versions: ObjectVersions = serde_cbor::from_slice(&read.value.context("no value")?)?;
    let (meta_v, ver_num, hash) = select_version_explicit(&versions, version)?;
    Ok((meta_v, ver_num, hash, read.meta.into()))
}

// -----------------------------------------------------------------------------=
// Misc helpers
// -----------------------------------------------------------------------------=

fn decode_cell_names_batch(metas: Vec<CellMeta>) -> Vec<String> {
    metas
        .into_iter()
        .map(|m| decode_key_bytes(&m.key_bytes))
        .collect()
}

fn decode_cell_names_control(entries: Vec<ClientCellEntry>) -> Vec<String> {
    entries
        .into_iter()
        .map(|e| {
            base64::engine::general_purpose::STANDARD
                .decode(&e.key_b64)
                .ok()
                .map(|b| decode_key_bytes(&b))
                .unwrap_or(e.key_b64)
        })
        .collect()
}

fn decode_key_bytes(bytes: &[u8]) -> String {
    serde_cbor::from_slice::<String>(bytes)
        .unwrap_or_else(|_| base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn build_obj_path(name: &str, version: Option<u64>, data: bool) -> String {
    let mut p = format!("/obj/{}", name);
    if let Some(v) = version {
        p.push_str(&format!("/v{}", v));
    }
    if data {
        p.push_str("/data");
    }
    p
}

fn select_version_explicit(
    versions: &ObjectVersions,
    requested: Option<u64>,
) -> Result<(ObjectMeta, u64, String)> {
    let ver = requested.unwrap_or_else(|| versions.latest);
    let meta = versions
        .versions
        .get(&ver)
        .cloned()
        .with_context(|| format!("version {ver} not found"))?;
    Ok((meta.clone(), ver, meta.hash.clone()))
}

fn select_version(
    _meta: &ObjectMeta,
    latest: u64,
    hash: String,
    requested: Option<u64>,
) -> (String, u64) {
    if let Some(v) = requested {
        (hash, v)
    } else {
        (hash, latest)
    }
}

fn matches_prefix(name: &str, parsed: &FsPath) -> bool {
    match parsed {
        FsPath::ObjMeta { name: prefix, .. } | FsPath::ObjData { name: prefix, .. } => {
            name.starts_with(prefix)
        }
        _ => true,
    }
}

fn cbor_key_bytes(name: &str) -> Result<Vec<u8>> {
    serde_cbor::to_vec(&name.to_string()).context("encode key")
}

fn key_bytes_opt(key: &Option<KeyRef>) -> Result<Option<Vec<u8>>> {
    match key {
        None => Ok(None),
        Some(KeyRef::Utf8(s)) => Ok(Some(s.as_bytes().to_vec())),
        Some(KeyRef::Hex(h)) => {
            let bytes = hex::decode(h)?;
            Ok(Some(bytes))
        }
    }
}

fn display_key(key: &KeyRef) -> String {
    match key {
        KeyRef::Utf8(s) => s.to_string(),
        KeyRef::Hex(h) => format!("0x{h}"),
    }
}
