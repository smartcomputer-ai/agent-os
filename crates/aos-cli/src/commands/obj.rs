//! `aos obj` commands built on the ObjectCatalog reducer.

use anyhow::Result;
use clap::{Args, Subcommand};
use base64::Engine;

use crate::key::{KeyOverrides, encode_key_for_reducer};
use crate::opts::{Mode, WorldOpts, resolve_dirs};
use crate::output::print_success;
use crate::util::load_world_env;

use super::{create_host, prepare_world, should_use_control, try_control_client};

const OBJECT_CATALOG: &str = "sys/ObjectCatalog@1";

#[derive(Args, Debug)]
pub struct ObjArgs {
    #[command(subcommand)]
    pub cmd: ObjCommand,
}

#[derive(Subcommand, Debug)]
pub enum ObjCommand {
    /// List object keys (names) with optional prefix
    Ls(ObjListArgs),
    /// Get object metadata/history
    Get(ObjGetArgs),
    /// Stat object (latest version)
    Stat(ObjStatArgs),
}

#[derive(Args, Debug)]
pub struct ObjListArgs {
    /// Name prefix filter
    pub prefix: Option<String>,
    /// Limit number of entries
    #[arg(long, default_value_t = 0)]
    pub limit: usize,
    /// Show all versions (requires ObjectCatalog support; ignored in batch fallback)
    #[arg(long)]
    pub versions: bool,
    /// Filter by kind (requires daemon/catalog)
    #[arg(long)]
    pub kind: Option<String>,
    /// Filter by tag (requires daemon/catalog)
    #[arg(long)]
    pub tag: Option<String>,
    /// Maximum path depth to include (e.g., 1 keeps `foo`, drops `foo/bar`)
    #[arg(long)]
    pub depth: Option<usize>,
}

#[derive(Args, Debug)]
pub struct ObjGetArgs {
    /// Object name (key)
    pub name: String,
    /// Specific version to fetch (default: latest)
    #[arg(long)]
    pub version: Option<u64>,
}

#[derive(Args, Debug)]
pub struct ObjStatArgs {
    /// Object name (key)
    pub name: String,
    /// Specific version to stat (default: latest)
    #[arg(long)]
    pub version: Option<u64>,
}

pub async fn cmd_obj(opts: &WorldOpts, args: &ObjArgs) -> Result<()> {
    match &args.cmd {
        ObjCommand::Ls(a) => obj_ls(opts, a).await,
        ObjCommand::Get(a) => obj_get(opts, a).await,
        ObjCommand::Stat(a) => obj_stat(opts, a).await,
    }
}

async fn obj_ls(opts: &WorldOpts, args: &ObjListArgs) -> Result<()> {
    // In v1, ObjectCatalog is keyed; we list keys via state ls equivalent (control list-cells).
    let dirs = resolve_dirs(opts)?;
    let mut warnings = Vec::new();
    if args.kind.is_some() || args.tag.is_some() || args.versions {
        warnings.push("kind/tag/versions filters require daemon-side catalog; ignored in batch".into());
    }
    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let (meta, cells) = client
                .list_cells_decoded("cli-obj-ls", OBJECT_CATALOG)
                .await?;
            let mut keys: Vec<String> = cells
                .into_iter()
                .filter_map(|c| base64::engine::general_purpose::STANDARD.decode(c.key_b64).ok())
                .filter_map(|bytes| serde_cbor::from_slice::<String>(&bytes).ok())
                .filter(|k| args.prefix.as_ref().map_or(true, |p| k.starts_with(p)))
                .filter(|k| within_depth(k, args.depth))
                .collect();
            if args.limit > 0 && keys.len() > args.limit {
                keys.truncate(args.limit);
            }
            return print_success(
                opts,
                serde_json::json!({ "objects": keys }),
                Some(meta_to_json(&meta)),
                warnings,
            );
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }
    // Batch/local read not supported for ObjectCatalog listing yet.
    warnings.push("object listing requires daemon/control; batch mode returns empty".into());
    print_success(
        opts,
        serde_json::json!([]),
        None,
        warnings,
    )
}

async fn obj_get(opts: &WorldOpts, args: &ObjGetArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let key_bytes = derive_key(&dirs, &args.name)?;
    let mut warnings = Vec::new();

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let (meta, state_opt) = client
                .query_state_decoded("cli-obj-get", OBJECT_CATALOG, Some(&key_bytes), None)
                .await?;
            let (data, warn) = select_version(state_opt, args.version)?;
            if let Some(w) = warn {
                warnings.push(w);
            }
            return print_success(opts, data, Some(meta_to_json(&meta)), warnings);
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }

    // Batch/local read
    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let host = create_host(store, loaded, &dirs, opts)?;
    if let Some(read) = host.query_state(
        OBJECT_CATALOG,
        Some(&key_bytes),
        aos_kernel::Consistency::Head,
    ) {
        let (data, warn) = select_version(read.value, args.version)?;
        if let Some(w) = warn {
            warnings.push(w);
        }
        return print_success(
            opts,
            data,
            Some(meta_to_json(&read.meta)),
            warnings
                .into_iter()
                .chain(std::iter::once("daemon unavailable; read via batch".into()))
                .collect(),
        );
    }

    print_success(
        opts,
        serde_json::json!(null),
        None,
        vec!["object not found".into()],
    )
}

async fn obj_stat(opts: &WorldOpts, args: &ObjStatArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let key_bytes = derive_key(&dirs, &args.name)?;
    let mut warnings = Vec::new();

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let (meta, state_opt) = client
                .query_state_decoded("cli-obj-stat", OBJECT_CATALOG, Some(&key_bytes), None)
                .await?;
            let (latest, warning) = state_opt
                .map(|bytes| select_version(Some(bytes), args.version))
                .transpose()?
                .unwrap_or((serde_json::json!(null), None));
            if let Some(w) = warning {
                warnings.push(w);
            }
            return print_success(
                opts,
                latest,
                Some(meta_to_json(&meta)),
                warnings,
            );
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }

    // Batch/local read
    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let host = create_host(store, loaded, &dirs, opts)?;
    if let Some(read) = host.query_state(
        OBJECT_CATALOG,
        Some(&key_bytes),
        aos_kernel::Consistency::Head,
    ) {
        let (latest, warning) = read
            .value
            .map(|bytes| select_version(Some(bytes), args.version))
            .transpose()?
            .unwrap_or((serde_json::json!(null), None));
        if let Some(w) = warning {
            warnings.push(w);
        }
        return print_success(
            opts,
            latest,
            Some(meta_to_json(&read.meta)),
            warnings
                .into_iter()
                .chain(if opts.quiet {
                    None
                } else {
                    Some("daemon unavailable; read via batch".into())
                })
                .collect(),
        );
    }

    print_success(
        opts,
        serde_json::json!(null),
        None,
        vec!["object not found".into()],
    )
}

fn derive_key(dirs: &crate::opts::ResolvedDirs, name: &str) -> Result<Vec<u8>> {
    let overrides = KeyOverrides {
        utf8: Some(name.to_string()),
        json: None,
        hex: None,
        b64: None,
    };
    encode_key_for_reducer(dirs, OBJECT_CATALOG, &overrides)
}

fn select_version(
    state_bytes: Option<Vec<u8>>,
    version: Option<u64>,
) -> Result<(serde_json::Value, Option<String>)> {
    let Some(bytes) = state_bytes else {
        return Ok((serde_json::json!(null), Some("object not found".into())));
    };
    let cbor_val: serde_cbor::Value = serde_cbor::from_slice(&bytes)?;
    let value = cbor_to_json_lossy(&cbor_val);
    let versions_map = value.get("versions").and_then(|v| v.as_object());
    // Helper to pull a version entry by number/string.
    let pick_version = |ver: u64, versions_map: &serde_json::Map<String, serde_json::Value>| {
        versions_map
            .get(&ver.to_string())
            .cloned()
            .ok_or_else(|| format!("version {ver} not found"))
    };

    // If a version is requested, honor it using versions map when present.
    if let Some(v) = version {
        if let Some(versions) = versions_map {
            match pick_version(v, versions) {
                Ok(entry) => return Ok((entry, None)),
                Err(msg) => {
                    return Ok((
                        serde_json::json!(null),
                        Some(format!("{msg}; use obj ls to view available versions")),
                    ))
                }
            }
        }
        // No versions map; return whole value with a warning.
        return Ok((
            value,
            Some("version selection unavailable; returned full state".into()),
        ));
    }

    // No version requested: prefer latest entry when versions map exists.
    if let Some(versions) = versions_map {
        if let Some(latest_val) = value.get("latest") {
            if let Some(latest_num) = latest_val.as_u64() {
                if let Ok(entry) = pick_version(latest_num, versions) {
                    return Ok((entry, None));
                }
            }
        }
        // Fallback: return the versions map itself.
        return Ok((serde_json::json!(versions), None));
    }

    // No versions map: return the decoded value.
    Ok((value, None))
}

fn cbor_to_json_lossy(val: &serde_cbor::Value) -> serde_json::Value {
    use serde_cbor::Value as Cbor;
    match val {
        Cbor::Null => serde_json::Value::Null,
        Cbor::Bool(b) => serde_json::Value::Bool(*b),
        Cbor::Integer(i) => {
            if let Ok(n) = i64::try_from(*i) {
                serde_json::json!(n)
            } else {
                serde_json::json!(i.to_string())
            }
        }
        Cbor::Bytes(b) => serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(b)),
        Cbor::Text(t) => serde_json::Value::String(t.clone()),
        Cbor::Array(a) => serde_json::Value::Array(a.iter().map(cbor_to_json_lossy).collect()),
        Cbor::Map(m) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in m {
                let key_str = match k {
                    Cbor::Text(t) => t.clone(),
                    Cbor::Integer(i) => i.to_string(),
                    other => format!("{}", cbor_to_json_lossy(other)),
                };
                obj.insert(key_str, cbor_to_json_lossy(v));
            }
            serde_json::Value::Object(obj)
        }
        Cbor::Tag(_, inner) => cbor_to_json_lossy(inner),
        Cbor::Float(f) => serde_json::Number::from_f64(*f).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null),
        _ => serde_json::Value::Null,
    }
}

fn meta_to_json(meta: &aos_kernel::ReadMeta) -> serde_json::Value {
    serde_json::json!({
        "journal_height": meta.journal_height,
        "snapshot_hash": meta.snapshot_hash.as_ref().map(|h| h.to_hex()),
        "manifest_hash": meta.manifest_hash.to_hex(),
    })
}

fn within_depth(key: &str, depth: Option<usize>) -> bool {
    match depth {
        None => true,
        Some(d) if d == 0 => true,
        Some(d) => key.split('/').count() <= d,
    }
}
