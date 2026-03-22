use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use aos_cbor::Hash;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use clap::{Args, Subcommand};
use serde_json::json;

use crate::GlobalOpts;
use crate::client::ApiClient;
use crate::output::{OutputOpts, print_success};

use super::common::{resolve_target, universe_id_for_world};

#[derive(Args, Debug)]
#[command(about = "Interact with the content-addressed blob store")]
pub(crate) struct CasArgs {
    #[command(subcommand)]
    cmd: CasCommand,
}

#[derive(Subcommand, Debug)]
enum CasCommand {
    /// Download a blob by hash.
    Get(CasGetArgs),
    /// Check whether a blob exists.
    Head(CasHeadArgs),
    /// Upload a blob from text or a file.
    Put(CasPutArgs),
}

#[derive(Args, Debug)]
struct CasGetArgs {
    /// Blob hash, with or without the `sha256:` prefix.
    sha256: String,
    /// Write the blob to a local file instead of stdout.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Optional world whose universe should scope the lookup.
    #[arg(long)]
    world: Option<String>,
}

#[derive(Args, Debug)]
struct CasHeadArgs {
    /// Blob hash, with or without the `sha256:` prefix.
    sha256: String,
    /// Optional world whose universe should scope the lookup.
    #[arg(long)]
    world: Option<String>,
}

#[derive(Args, Debug)]
struct CasPutArgs {
    /// Read blob bytes from a file.
    #[arg(long)]
    file: Option<PathBuf>,
    /// Upload inline text as the blob body.
    #[arg(long)]
    text: Option<String>,
    /// Optional world whose universe should scope the upload.
    #[arg(long)]
    world: Option<String>,
}

pub(crate) async fn handle(global: &GlobalOpts, output: OutputOpts, args: CasArgs) -> Result<()> {
    let target = resolve_target(global)?;
    let client = ApiClient::new(&target)?;
    match args.cmd {
        CasCommand::Get(args) => {
            let query = match args.world.as_deref() {
                Some(world) => vec![(
                    "universe_id",
                    Some(universe_id_for_world(&client, world).await?),
                )],
                None => Vec::new(),
            };
            let bytes = client
                .get_bytes(&format!("/v1/cas/blobs/{}", args.sha256), &query)
                .await?;
            if let Some(path) = args.out {
                fs::write(&path, &bytes).with_context(|| format!("write {}", path.display()))?;
                print_success(
                    output,
                    json!({ "path": path.display().to_string(), "bytes": bytes.len() }),
                    None,
                    vec![],
                )
            } else {
                print_success(
                    output,
                    json!({ "data_b64": BASE64_STANDARD.encode(bytes) }),
                    None,
                    vec![],
                )
            }
        }
        CasCommand::Head(args) => {
            let path = match args.world.as_deref() {
                Some(world) => format!(
                    "/v1/cas/blobs/{}?universe_id={}",
                    args.sha256,
                    universe_id_for_world(&client, world).await?
                ),
                None => format!("/v1/cas/blobs/{}", args.sha256),
            };
            let exists = client.head_exists(&path).await?;
            print_success(output, json!({ "exists": exists }), None, vec![])
        }
        CasCommand::Put(args) => {
            let bytes = match (args.text, args.file) {
                (Some(text), None) => text.into_bytes(),
                (None, Some(path)) => {
                    fs::read(&path).with_context(|| format!("read {}", path.display()))?
                }
                _ => return Err(anyhow!("cas put requires exactly one of --text or --file")),
            };
            let hash = Hash::of_bytes(&bytes).to_hex();
            let path = match args.world.as_deref() {
                Some(world) => format!(
                    "/v1/cas/blobs/{hash}?universe_id={}",
                    universe_id_for_world(&client, world).await?
                ),
                None => format!("/v1/cas/blobs/{hash}"),
            };
            let data = client.put_bytes(&path, bytes).await?;
            print_success(output, data, None, vec![])
        }
    }
}
