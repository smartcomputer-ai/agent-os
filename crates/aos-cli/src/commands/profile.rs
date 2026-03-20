use anyhow::{Result, anyhow};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use crate::GlobalOpts;
use crate::config::{
    ConfigPaths, ProfileConfig, ProfileKind, load_config, redact_profile, save_config,
};
use crate::output::{OutputOpts, print_success};

#[derive(Args, Debug)]
#[command(about = "Manage saved CLI profiles and the current default selectors")]
pub(crate) struct ProfileArgs {
    #[command(subcommand)]
    cmd: ProfileCommand,
}

#[derive(Subcommand, Debug)]
enum ProfileCommand {
    /// Show the current profile, if selected.
    Show,
    /// List all saved profiles.
    Ls,
    /// Add or replace a saved profile.
    Add(ProfileAddArgs),
    /// Remove a saved profile.
    Rm(ProfileRemoveArgs),
    /// Make a saved profile current.
    Select(ProfileSelectArgs),
    /// Update saved profile fields.
    Set(ProfileSetArgs),
    /// Clear the current profile selection or saved selectors.
    Clear(ProfileClearArgs),
}

#[derive(Args, Debug)]
struct ProfileAddArgs {
    /// Profile name to create or update.
    name: String,
    /// Control API base URL for this profile.
    #[arg(long)]
    api: String,
    /// Bearer token to store in the profile.
    #[arg(long)]
    token: Option<String>,
    /// Environment variable that contains the bearer token.
    #[arg(long = "token-env")]
    token_env: Option<String>,
    /// Custom HTTP header in `KEY=VALUE` form.
    #[arg(long)]
    header: Vec<String>,
    /// Profile kind.
    #[arg(long, default_value = "remote")]
    kind: String,
}

#[derive(Args, Debug)]
struct ProfileRemoveArgs {
    /// Profile name to remove.
    name: String,
}

#[derive(Args, Debug)]
struct ProfileSetArgs {
    /// Profile name to update. Defaults to the current profile.
    #[arg(long)]
    profile: Option<String>,
    /// Default universe selector for the profile.
    #[arg(long)]
    universe: Option<String>,
    /// Default world selector for the profile.
    #[arg(long)]
    world: Option<String>,
    /// Profile kind.
    #[arg(long)]
    kind: Option<String>,
}

#[derive(Args, Debug)]
struct ProfileSelectArgs {
    /// Profile name to make current.
    profile: String,
}

#[derive(Args, Debug)]
struct ProfileClearArgs {
    /// Profile name to update when clearing saved selectors. Defaults to the current profile.
    #[arg(long)]
    profile: Option<String>,
    /// Clear only the default universe selector.
    #[arg(long = "clear-universe")]
    clear_universe: bool,
    /// Clear only the default world selector.
    #[arg(long = "clear-world")]
    clear_world: bool,
}

pub(crate) async fn handle(
    global: &GlobalOpts,
    output: OutputOpts,
    args: ProfileArgs,
) -> Result<()> {
    let paths = ConfigPaths::resolve(global.config.as_deref())?;
    let mut config = load_config(&paths)?;
    match args.cmd {
        ProfileCommand::Show => {
            let Some(current_profile) = config.current_profile.as_deref() else {
                return Ok(());
            };
            let Some(profile) = config.profiles.get(current_profile) else {
                return Ok(());
            };
            let data = json!({
                "name": current_profile,
                "profile": redact_profile(profile),
            });
            print_success(output, data, None, vec![])
        }
        ProfileCommand::Ls => {
            let data = Value::Array(
                config
                    .profiles
                    .iter()
                    .map(|(name, profile)| {
                        json!({
                            "name": name,
                            "selected": config.current_profile.as_deref() == Some(name.as_str()),
                            "profile": redact_profile(profile),
                        })
                    })
                    .collect(),
            );
            print_success(output, data, None, vec![])
        }
        ProfileCommand::Add(args) => {
            let mut headers = parse_headers(&args.header)?;
            for (key, value) in parse_headers(&global.header)? {
                headers.entry(key).or_insert(value);
            }
            let kind = args.kind.parse::<ProfileKind>()?;
            config.profiles.insert(
                args.name.clone(),
                ProfileConfig {
                    kind,
                    api: args.api,
                    token: args.token,
                    token_env: args.token_env,
                    headers,
                    universe: None,
                    world: None,
                },
            );
            if config.current_profile.is_none() {
                config.current_profile = Some(args.name);
            }
            save_config(&paths, &config)?;
            print_success(output, json!({ "status": "saved" }), None, vec![])
        }
        ProfileCommand::Rm(args) => {
            config.profiles.remove(&args.name);
            if config.current_profile.as_deref() == Some(args.name.as_str()) {
                config.current_profile = None;
            }
            save_config(&paths, &config)?;
            print_success(output, json!({ "status": "removed" }), None, vec![])
        }
        ProfileCommand::Select(args) => {
            if !config.profiles.contains_key(&args.profile) {
                return Err(anyhow!("profile '{}' not found", args.profile));
            }
            config.current_profile = Some(args.profile);
            save_config(&paths, &config)?;
            print_success(output, json!({ "status": "selected" }), None, vec![])
        }
        ProfileCommand::Set(args) => {
            if args.universe.is_none() && args.world.is_none() && args.kind.is_none() {
                return Err(anyhow!(
                    "profile set requires at least one of --universe, --world, or --kind; use `aos profile select <name>` to change the current profile"
                ));
            }
            let profile_name = args
                .profile
                .or_else(|| config.current_profile.clone())
                .ok_or_else(|| anyhow!("no profile selected; set --profile"))?;
            let profile = config
                .profiles
                .get_mut(&profile_name)
                .ok_or_else(|| anyhow!("profile '{profile_name}' not found"))?;
            if let Some(universe) = args.universe {
                profile.universe = Some(universe);
            }
            if let Some(world) = args.world {
                profile.world = Some(world);
            }
            if let Some(kind) = args.kind {
                profile.kind = kind.parse::<ProfileKind>()?;
            }
            save_config(&paths, &config)?;
            print_success(output, json!({ "status": "updated" }), None, vec![])
        }
        ProfileCommand::Clear(args) => {
            if !args.clear_universe && !args.clear_world {
                config.current_profile = None;
            } else {
                let profile_name = args
                    .profile
                    .or_else(|| config.current_profile.clone())
                    .ok_or_else(|| anyhow!("no profile selected; set --profile"))?;
                let profile = config
                    .profiles
                    .get_mut(&profile_name)
                    .ok_or_else(|| anyhow!("profile '{profile_name}' not found"))?;
                if args.clear_universe {
                    profile.universe = None;
                }
                if args.clear_world {
                    profile.world = None;
                }
            }
            save_config(&paths, &config)?;
            print_success(output, json!({ "status": "cleared" }), None, vec![])
        }
    }
}

fn parse_headers(values: &[String]) -> Result<std::collections::BTreeMap<String, String>> {
    let mut headers = std::collections::BTreeMap::new();
    for value in values {
        let (name, header_value) = value
            .split_once('=')
            .ok_or_else(|| anyhow!("headers must be KEY=VALUE: {value}"))?;
        headers.insert(name.to_string(), header_value.to_string());
    }
    Ok(headers)
}
