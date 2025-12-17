//! `aos blob` commands.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use aos_store::Store;

use crate::input::parse_input_bytes;
use crate::opts::{Mode, WorldOpts, resolve_dirs};
use crate::output::print_success;
use crate::util::load_world_env;

use super::{should_use_control, try_control_client};

#[derive(Args, Debug)]
pub struct BlobArgs {
    #[command(subcommand)]
    pub cmd: BlobCommand,
}

#[derive(Subcommand, Debug)]
pub enum BlobCommand {
    /// Put a blob into the store/CAS
    Put(BlobPutArgs),
    /// Get a blob by hash
    Get(BlobGetArgs),
    /// Stat a blob (existence/size)
    Stat(BlobStatArgs),
}

#[derive(Args, Debug)]
pub struct BlobPutArgs {
    /// Path to file or @- for stdin
    pub input: String,
}

#[derive(Args, Debug)]
pub struct BlobGetArgs {
    /// Blob hash (hex)
    pub hash: String,
    /// Write raw bytes to stdout instead of metadata
    #[arg(long)]
    pub raw: bool,
    /// Output file path
    #[arg(long)]
    pub out: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct BlobStatArgs {
    /// Blob hash (hex)
    pub hash: String,
}

pub async fn cmd_blob(opts: &WorldOpts, args: &BlobArgs) -> Result<()> {
    match &args.cmd {
        BlobCommand::Put(a) => blob_put(opts, a).await,
        BlobCommand::Get(a) => blob_get(opts, a).await,
        BlobCommand::Stat(a) => blob_stat(opts, a).await,
    }
}

async fn blob_put(opts: &WorldOpts, args: &BlobPutArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let data = parse_input_bytes(&args.input)?;

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let payload = B64.encode(&data);
            let resp = client
                .request(&aos_host::control::RequestEnvelope {
                    v: 1,
                    id: "cli-blob-put".into(),
                    cmd: "blob-put".into(),
                    payload: serde_json::json!({ "data_b64": payload }),
                })
                .await?;
            if !resp.ok {
                anyhow::bail!(
                    "blob put failed: {}",
                    resp.error
                        .map(|e| format!("{}: {}", e.code, e.message))
                        .unwrap_or_else(|| "unknown error".into())
                );
            }
            let hash = resp
                .result
                .and_then(|v| v.get("hash").and_then(|h| h.as_str()).map(|s| s.to_string()))
                .unwrap_or_default();
            return print_success(opts, serde_json::json!({ "hash": hash }), None, vec![]);
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
    }

    // Local write
    load_world_env(&dirs.world)?;
    let store = Arc::new(aos_store::FsStore::open(&dirs.store_root)?);
    let hash = store.put_blob(&data)?;
    print_success(
        opts,
        serde_json::json!({ "hash": hash.to_hex() }),
        None,
        if opts.quiet {
            vec![]
        } else {
            vec!["daemon unavailable; wrote blob locally".into()]
        },
    )
}

async fn blob_get(opts: &WorldOpts, args: &BlobGetArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let hash_text = normalize_hash(&args.hash);
    let data = if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let resp = client.blob_get("cli-blob-get", &hash_text).await?;
            return output_blob(opts, &resp, args);
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        }
        None
    } else {
        None
    };

    let data = if let Some(d) = data {
        d
    } else {
        load_world_env(&dirs.world)?;
        let store = Arc::new(aos_store::FsStore::open(&dirs.store_root)?);
        let hash = aos_cbor::Hash::from_hex_str(&hash_text)
            .with_context(|| format!("invalid hash: {}", hash_text))?;
        store.get_blob(hash)?
    };
    output_blob(opts, &data, args)
}

fn output_blob(opts: &WorldOpts, data: &[u8], args: &BlobGetArgs) -> Result<()> {
    if let Some(out) = &args.out {
        fs::write(out, data)?;
        return print_success(
            opts,
            serde_json::json!({ "written": out, "bytes": data.len() }),
            None,
            vec![],
        );
    }
    if args.raw {
        let mut stdout = std::io::stdout();
        use std::io::Write;
        stdout.write_all(data)?;
        stdout.flush()?;
        return Ok(());
    }
    print_success(
        opts,
        serde_json::json!({
            "bytes": data.len(),
            "hint": "use --raw or --out to write bytes"
        }),
        None,
        vec!["blob get default is metadata-only; pass --raw or --out to emit bytes".into()],
    )
}

fn normalize_hash(input: &str) -> String {
    if input.starts_with("sha256:") {
        input.to_string()
    } else {
        format!("sha256:{input}")
    }
}


async fn blob_stat(opts: &WorldOpts, args: &BlobStatArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    load_world_env(&dirs.world)?;
    let hash = aos_cbor::Hash::from_hex_str(&args.hash)
        .with_context(|| format!("invalid hash: {}", args.hash))?;
    let path = dirs
        .store_root
        .join(".aos/blobs")
        .join(hash.to_hex());
    if !path.exists() {
        return print_success(opts, serde_json::json!({ "exists": false }), None, vec![]);
    }
    let meta = fs::metadata(&path)?;
    print_success(
        opts,
        serde_json::json!({
            "exists": true,
            "bytes": meta.len(),
            "path": path,
        }),
        None,
        vec![],
    )
}
