use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use clap::{Args, Subcommand};
use serde_json::json;

use crate::GlobalOpts;
use crate::authoring::sync_node_secrets;
use crate::client::ApiClient;
use crate::output::{OutputOpts, print_success};

use super::common::{encode_path_segment, resolve_target};

#[derive(Args, Debug)]
#[command(about = "Manage node secret bindings and secret values")]
pub(crate) struct UniverseArgs {
    #[command(subcommand)]
    cmd: UniverseCommand,
}

#[derive(Subcommand, Debug)]
enum UniverseCommand {
    /// Manage secret bindings and secret versions.
    Secret(UniverseSecretArgs),
}

#[derive(Args, Debug)]
#[command(about = "Manage node secret bindings and secret values")]
struct UniverseSecretArgs {
    /// Optional universe UUID. Defaults to the shared default domain.
    #[arg(long)]
    universe_id: Option<String>,
    #[command(subcommand)]
    cmd: UniverseSecretCommand,
}

#[derive(Subcommand, Debug)]
enum UniverseSecretCommand {
    /// Manage secret bindings.
    Binding(UniverseSecretBindingArgs),
    /// Manage secret versions for a binding.
    Version(UniverseSecretVersionArgs),
    /// Sync secret bindings and values from `aos.world.json`.
    Sync(UniverseSecretSyncArgs),
}

#[derive(Args, Debug)]
#[command(about = "Manage secret binding metadata")]
struct UniverseSecretBindingArgs {
    #[command(subcommand)]
    cmd: UniverseSecretBindingCommand,
}

#[derive(Subcommand, Debug)]
enum UniverseSecretBindingCommand {
    /// List secret bindings in the selected universe.
    Ls,
    /// Show one secret binding.
    Get(UniverseSecretBindingGetArgs),
    /// Create or update a secret binding.
    Set(UniverseSecretBindingSetArgs),
    /// Disable a secret binding.
    Delete(UniverseSecretBindingDeleteArgs),
}

#[derive(Args, Debug)]
struct UniverseSecretBindingGetArgs {
    binding_id: String,
}

#[derive(Args, Debug)]
struct UniverseSecretBindingSetArgs {
    binding_id: String,
    source_kind: String,
    #[arg(long)]
    env_var: Option<String>,
    #[arg(long)]
    required_placement_pin: Option<String>,
    #[arg(long)]
    status: Option<String>,
    #[arg(long)]
    actor: Option<String>,
}

#[derive(Args, Debug)]
struct UniverseSecretBindingDeleteArgs {
    binding_id: String,
}

#[derive(Args, Debug)]
#[command(about = "Manage encrypted secret versions for a binding")]
struct UniverseSecretVersionArgs {
    #[command(subcommand)]
    cmd: UniverseSecretVersionCommand,
}

#[derive(Subcommand, Debug)]
enum UniverseSecretVersionCommand {
    /// Add a new secret version from text or a file.
    Add(UniverseSecretVersionAddArgs),
    /// List versions for a binding.
    Ls(UniverseSecretVersionListArgs),
    /// Show one secret version.
    Get(UniverseSecretVersionGetArgs),
}

#[derive(Args, Debug)]
struct UniverseSecretVersionAddArgs {
    binding_id: String,
    #[arg(long)]
    text: Option<String>,
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long)]
    expected_digest: Option<String>,
    #[arg(long)]
    actor: Option<String>,
}

#[derive(Args, Debug)]
struct UniverseSecretVersionListArgs {
    binding_id: String,
}

#[derive(Args, Debug)]
struct UniverseSecretVersionGetArgs {
    binding_id: String,
    version: u64,
}

#[derive(Args, Debug)]
struct UniverseSecretSyncArgs {
    #[arg(long)]
    local_root: Option<PathBuf>,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    actor: Option<String>,
}

pub(crate) async fn handle(
    global: &GlobalOpts,
    output: OutputOpts,
    args: UniverseArgs,
) -> Result<()> {
    let target = resolve_target(global)?;
    let client = ApiClient::new(&target)?;
    match args.cmd {
        UniverseCommand::Secret(secret_args) => {
            let universe_id = secret_args.universe_id.as_deref();
            match secret_args.cmd {
                UniverseSecretCommand::Binding(args) => {
                    handle_binding(&client, output, universe_id, args).await
                }
                UniverseSecretCommand::Version(args) => {
                    handle_version(&client, output, universe_id, args).await
                }
                UniverseSecretCommand::Sync(args) => {
                    let data = sync_node_secrets(
                        &client,
                        universe_id,
                        args.local_root.as_deref(),
                        args.config.as_deref(),
                        args.actor.as_deref(),
                    )
                    .await?;
                    print_success(output, data, None, vec![])
                }
            }
        }
    }
}

async fn handle_binding(
    client: &ApiClient,
    output: OutputOpts,
    universe_id: Option<&str>,
    args: UniverseSecretBindingArgs,
) -> Result<()> {
    match args.cmd {
        UniverseSecretBindingCommand::Ls => {
            let data = client
                .get_json("/v1/secrets/bindings", &universe_query(universe_id))
                .await?;
            print_success(output, data, None, vec![])
        }
        UniverseSecretBindingCommand::Get(args) => {
            let data = client
                .get_json(
                    &format!(
                        "/v1/secrets/bindings/{}",
                        encode_path_segment(&args.binding_id)
                    ),
                    &universe_query(universe_id),
                )
                .await?;
            print_success(output, data, None, vec![])
        }
        UniverseSecretBindingCommand::Set(args) => {
            let status = args.status.unwrap_or_else(|| "active".to_string());
            let data = client
                .put_json(
                    &with_universe_query(
                        &format!(
                            "/v1/secrets/bindings/{}",
                            encode_path_segment(&args.binding_id)
                        ),
                        universe_id,
                    ),
                    &json!({
                        "source_kind": args.source_kind,
                        "env_var": args.env_var,
                        "required_placement_pin": args.required_placement_pin,
                        "status": status,
                        "actor": args.actor,
                    }),
                )
                .await?;
            print_success(output, data, None, vec![])
        }
        UniverseSecretBindingCommand::Delete(args) => {
            let data = client
                .delete_json(
                    &format!(
                        "/v1/secrets/bindings/{}",
                        encode_path_segment(&args.binding_id)
                    ),
                    &universe_query(universe_id),
                )
                .await?;
            print_success(output, data, None, vec![])
        }
    }
}

async fn handle_version(
    client: &ApiClient,
    output: OutputOpts,
    universe_id: Option<&str>,
    args: UniverseSecretVersionArgs,
) -> Result<()> {
    match args.cmd {
        UniverseSecretVersionCommand::Add(args) => {
            let plaintext = match (args.text, args.file) {
                (Some(text), None) => text.into_bytes(),
                (None, Some(path)) => {
                    fs::read(&path).with_context(|| format!("read {}", path.display()))?
                }
                _ => {
                    return Err(anyhow!(
                        "secret version add requires exactly one of --text or --file"
                    ));
                }
            };
            let data = client
                .post_json(
                    &with_universe_query(
                        &format!(
                            "/v1/secrets/bindings/{}/versions",
                            encode_path_segment(&args.binding_id)
                        ),
                        universe_id,
                    ),
                    &json!({
                        "plaintext_b64": base64::engine::general_purpose::STANDARD.encode(plaintext),
                        "expected_digest": args.expected_digest,
                        "actor": args.actor,
                    }),
                )
                .await?;
            print_success(output, data, None, vec![])
        }
        UniverseSecretVersionCommand::Ls(args) => {
            let data = client
                .get_json(
                    &format!(
                        "/v1/secrets/bindings/{}/versions",
                        encode_path_segment(&args.binding_id)
                    ),
                    &universe_query(universe_id),
                )
                .await?;
            print_success(output, data, None, vec![])
        }
        UniverseSecretVersionCommand::Get(args) => {
            let data = client
                .get_json(
                    &format!(
                        "/v1/secrets/bindings/{}/versions/{}",
                        encode_path_segment(&args.binding_id),
                        args.version
                    ),
                    &universe_query(universe_id),
                )
                .await?;
            print_success(output, data, None, vec![])
        }
    }
}

fn universe_query(universe_id: Option<&str>) -> Vec<(&'static str, Option<String>)> {
    vec![(
        "universe_id",
        universe_id.map(std::string::ToString::to_string),
    )]
}

fn with_universe_query(path: &str, universe_id: Option<&str>) -> String {
    match universe_id {
        Some(universe_id) => format!("{path}?universe_id={universe_id}"),
        None => path.to_string(),
    }
}
