use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use clap::{Args, Subcommand};
use serde_json::json;

use crate::GlobalOpts;
use crate::authoring::sync_hosted_secrets;
use crate::client::ApiClient;
use crate::config::{ConfigPaths, load_config, save_config};
use crate::output::{OutputOpts, print_success};

use super::common::{
    encode_path_segment, resolve_selected_universe, resolve_target,
    resolve_universe_arg_or_selected,
};

#[derive(Args, Debug)]
#[command(about = "Manage universes and universe-scoped resources")]
pub(crate) struct UniverseArgs {
    #[command(subcommand)]
    cmd: UniverseCommand,
}

#[derive(Subcommand, Debug)]
enum UniverseCommand {
    /// List universes.
    Ls,
    /// Show one universe by selector or the selected default.
    Get(UniverseGetArgs),
    /// Create a universe.
    Create(UniverseCreateArgs),
    /// Update universe metadata.
    Set(UniverseSetArgs),
    /// Delete a universe.
    Delete(UniverseDeleteArgs),
    /// Manage secret bindings and secret versions.
    Secret(UniverseSecretArgs),
}

#[derive(Args, Debug)]
struct UniverseGetArgs {
    /// Universe ID or handle. Defaults to the selected universe.
    selector: Option<String>,
}

#[derive(Args, Debug)]
struct UniverseCreateArgs {
    /// Explicit universe ID to create.
    #[arg(long)]
    universe_id: Option<String>,
    /// Human-readable handle for the new universe.
    #[arg(long)]
    handle: Option<String>,
    /// Make the created universe the selected universe on the current CLI profile.
    #[arg(long)]
    select: bool,
}

#[derive(Args, Debug)]
struct UniverseSetArgs {
    /// Universe ID or handle. Defaults to the selected universe.
    selector: Option<String>,
    /// New handle for the universe.
    #[arg(long)]
    handle: Option<String>,
}

#[derive(Args, Debug)]
struct UniverseDeleteArgs {
    /// Universe ID or handle. Defaults to the selected universe.
    selector: Option<String>,
}

#[derive(Args, Debug)]
#[command(about = "Manage universe-scoped secret bindings and secret values")]
struct UniverseSecretArgs {
    #[command(subcommand)]
    cmd: UniverseSecretCommand,
}

#[derive(Subcommand, Debug)]
enum UniverseSecretCommand {
    /// Manage secret bindings.
    Binding(UniverseSecretBindingArgs),
    /// Manage secret versions for a binding.
    Version(UniverseSecretVersionArgs),
    /// Sync secret bindings and values from `aos.sync.json`.
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
    /// Binding identifier.
    binding_id: String,
}

#[derive(Args, Debug)]
struct UniverseSecretBindingSetArgs {
    /// Binding identifier.
    binding_id: String,
    /// Binding source kind, such as `env_var`.
    source_kind: String,
    /// Environment variable name when `source_kind=env_var`.
    #[arg(long)]
    env_var: Option<String>,
    /// Placement pin required to resolve this binding.
    #[arg(long)]
    required_placement_pin: Option<String>,
    /// Optional binding lifecycle status override.
    #[arg(long)]
    status: Option<String>,
    /// Actor string recorded in audit history.
    #[arg(long)]
    actor: Option<String>,
}

#[derive(Args, Debug)]
struct UniverseSecretBindingDeleteArgs {
    /// Binding identifier.
    binding_id: String,
    /// Actor string recorded in audit history.
    #[arg(long)]
    actor: Option<String>,
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
    /// Binding identifier.
    binding_id: String,
    /// Secret plaintext provided inline.
    #[arg(long)]
    text: Option<String>,
    /// File that contains the secret plaintext.
    #[arg(long)]
    file: Option<PathBuf>,
    /// Expected digest to enforce when writing the version.
    #[arg(long)]
    expected_digest: Option<String>,
    /// Actor string recorded in audit history.
    #[arg(long)]
    actor: Option<String>,
}

#[derive(Args, Debug)]
struct UniverseSecretVersionListArgs {
    /// Binding identifier.
    binding_id: String,
}

#[derive(Args, Debug)]
struct UniverseSecretVersionGetArgs {
    /// Binding identifier.
    binding_id: String,
    /// Version number to fetch.
    version: u64,
}

#[derive(Args, Debug)]
struct UniverseSecretSyncArgs {
    /// Local world root that contains `aos.sync.json`.
    #[arg(long)]
    local_root: Option<PathBuf>,
    /// Explicit sync map path. Defaults to `aos.sync.json`.
    #[arg(long)]
    map: Option<PathBuf>,
    /// Actor string recorded in audit history.
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
        UniverseCommand::Ls => {
            let data = client.get_json("/v1/universes", &[]).await?;
            print_success(output, data, None, vec![])
        }
        UniverseCommand::Get(args) => {
            let universe =
                resolve_universe_arg_or_selected(&client, &target, args.selector.as_deref())
                    .await?;
            let data = client
                .get_json(&format!("/v1/universes/{universe}"), &[])
                .await?;
            print_success(output, data, None, vec![])
        }
        UniverseCommand::Create(args) => {
            let data = client
                .post_json(
                    "/v1/universes",
                    &json!({
                        "universe_id": args.universe_id,
                        "handle": args.handle,
                    }),
                )
                .await?;
            if args.select {
                let universe_id = created_universe_id(&data)?;
                select_created_universe(global, &universe_id)?;
            }
            print_success(output, data, None, vec![])
        }
        UniverseCommand::Set(args) => {
            let universe =
                resolve_universe_arg_or_selected(&client, &target, args.selector.as_deref())
                    .await?;
            let data = client
                .patch_json(
                    &format!("/v1/universes/{universe}"),
                    &json!({ "handle": args.handle }),
                )
                .await?;
            print_success(output, data, None, vec![])
        }
        UniverseCommand::Delete(args) => {
            let universe =
                resolve_universe_arg_or_selected(&client, &target, args.selector.as_deref())
                    .await?;
            let data = client
                .delete_json(&format!("/v1/universes/{universe}"), &[])
                .await?;
            print_success(output, data, None, vec![])
        }
        UniverseCommand::Secret(args) => {
            let universe = resolve_selected_universe(&client, &target).await?;
            handle_secret(&client, output, &universe, args).await
        }
    }
}

fn created_universe_id(data: &serde_json::Value) -> Result<String> {
    data.get("record")
        .and_then(|value| value.get("universe_id"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("universe create response did not include record.universe_id"))
}

fn select_created_universe(global: &GlobalOpts, universe_id: &str) -> Result<()> {
    let paths = ConfigPaths::resolve(global.config.as_deref())?;
    let mut config = load_config(&paths)?;
    let profile_name = global
        .profile
        .clone()
        .or_else(|| config.current_profile.clone())
        .ok_or_else(|| {
            anyhow!("cannot --select created universe without a saved current profile")
        })?;
    let profile = config
        .profiles
        .get_mut(&profile_name)
        .ok_or_else(|| anyhow!("profile '{profile_name}' not found"))?;
    profile.universe = Some(universe_id.to_string());
    profile.world = None;
    config.current_profile = Some(profile_name);
    save_config(&paths, &config)
}

async fn handle_secret(
    client: &ApiClient,
    output: OutputOpts,
    universe: &str,
    args: UniverseSecretArgs,
) -> Result<()> {
    match args.cmd {
        UniverseSecretCommand::Binding(args) => match args.cmd {
            UniverseSecretBindingCommand::Ls => {
                let data = client
                    .get_json(&format!("/v1/universes/{universe}/secrets/bindings"), &[])
                    .await?;
                print_success(output, data, None, vec![])
            }
            UniverseSecretBindingCommand::Get(args) => {
                let binding_id = encode_path_segment(&args.binding_id);
                let data = client
                    .get_json(
                        &format!("/v1/universes/{universe}/secrets/bindings/{}", binding_id),
                        &[],
                    )
                    .await?;
                print_success(output, data, None, vec![])
            }
            UniverseSecretBindingCommand::Set(args) => {
                let binding_id = encode_path_segment(&args.binding_id);
                let data = client
                    .put_json(
                        &format!("/v1/universes/{universe}/secrets/bindings/{}", binding_id),
                        &json!({
                            "source_kind": args.source_kind,
                            "env_var": args.env_var,
                            "required_placement_pin": args.required_placement_pin,
                            "status": args.status,
                            "actor": args.actor,
                        }),
                    )
                    .await?;
                print_success(output, data, None, vec![])
            }
            UniverseSecretBindingCommand::Delete(args) => {
                let binding_id = encode_path_segment(&args.binding_id);
                let data = client
                    .delete_json(
                        &format!("/v1/universes/{universe}/secrets/bindings/{}", binding_id),
                        &[("actor", args.actor)],
                    )
                    .await?;
                print_success(output, data, None, vec![])
            }
        },
        UniverseSecretCommand::Version(args) => match args.cmd {
            UniverseSecretVersionCommand::Add(args) => {
                let binding_id = encode_path_segment(&args.binding_id);
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
                        &format!(
                            "/v1/universes/{universe}/secrets/bindings/{}/versions",
                            binding_id
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
                let binding_id = encode_path_segment(&args.binding_id);
                let data = client
                    .get_json(
                        &format!(
                            "/v1/universes/{universe}/secrets/bindings/{}/versions",
                            binding_id
                        ),
                        &[],
                    )
                    .await?;
                print_success(output, data, None, vec![])
            }
            UniverseSecretVersionCommand::Get(args) => {
                let binding_id = encode_path_segment(&args.binding_id);
                let data = client
                    .get_json(
                        &format!(
                            "/v1/universes/{universe}/secrets/bindings/{}/versions/{}",
                            binding_id, args.version
                        ),
                        &[],
                    )
                    .await?;
                print_success(output, data, None, vec![])
            }
        },
        UniverseSecretCommand::Sync(args) => {
            let data = sync_hosted_secrets(
                client,
                universe,
                args.local_root.as_deref(),
                args.map.as_deref(),
                args.actor.as_deref(),
            )
            .await?;
            print_success(output, data, None, vec![])
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use crate::config::{CliConfig, ProfileConfig, ProfileKind};

    use super::*;

    fn test_global_opts(config: &std::path::Path) -> GlobalOpts {
        GlobalOpts {
            profile: None,
            api: None,
            token: None,
            header: Vec::new(),
            universe: None,
            world: None,
            config: Some(config.to_path_buf()),
            json: false,
            pretty: false,
            quiet: false,
            no_meta: false,
            verbose: false,
        }
    }

    #[test]
    fn created_universe_id_reads_create_response() {
        let universe_id = created_universe_id(&json!({
            "record": {
                "universe_id": "u-123"
            }
        }))
        .expect("created universe id");
        assert_eq!(universe_id, "u-123");
    }

    #[test]
    fn select_created_universe_updates_profile_and_clears_world() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let config_path = temp.path().join("cli.json");
        let paths = ConfigPaths {
            path: config_path.clone(),
        };
        let mut profiles = BTreeMap::new();
        profiles.insert(
            "remote".into(),
            ProfileConfig {
                kind: ProfileKind::Remote,
                api: "http://127.0.0.1:9080".into(),
                token: None,
                token_env: None,
                headers: BTreeMap::new(),
                universe: Some("old-u".into()),
                world: Some("old-w".into()),
            },
        );
        save_config(
            &paths,
            &CliConfig {
                current_profile: Some("remote".into()),
                profiles,
            },
        )
        .expect("save config");

        select_created_universe(&test_global_opts(&config_path), "new-u")
            .expect("select created universe");

        let config = load_config(&paths).expect("load config");
        assert_eq!(config.current_profile.as_deref(), Some("remote"));
        let profile = config.profiles.get("remote").expect("remote profile");
        assert_eq!(profile.universe.as_deref(), Some("new-u"));
        assert_eq!(profile.world, None);
    }
}
