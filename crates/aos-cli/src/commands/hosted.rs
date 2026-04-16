use std::fs;
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use aos_node::UniverseId;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::GlobalOpts;
use crate::config::{ConfigPaths, ProfileConfig, ProfileKind, load_config, save_config};
use crate::output::{OutputOpts, print_success};

const DEFAULT_HOSTED_PROFILE: &str = "hosted";
const DEFAULT_HOSTED_BIND: &str = "127.0.0.1:9011";
const DEFAULT_HOSTED_STARTUP_WAIT_MS: u64 = 30_000;

#[derive(Args, Debug)]
#[command(about = "Manage the locally hosted AgentOS node")]
pub(crate) struct HostedArgs {
    #[command(subcommand)]
    cmd: HostedCommand,
}

#[derive(Subcommand, Debug)]
enum HostedCommand {
    /// Start the locally hosted node and ensure the hosted CLI profile points at it.
    Up(HostedUpArgs),
    /// Show hosted node process and health status.
    Status(HostedRuntimeArgs),
    /// Stop the hosted node process managed from this runtime home.
    Down(HostedDownArgs),
    /// Select the reserved hosted profile as the current CLI profile.
    Use(HostedUseArgs),
}

#[derive(Args, Debug, Clone)]
struct HostedRuntimeArgs {
    /// Override the hosted runtime home directory.
    #[arg(long, env = "AOS_HOSTED_ROOT")]
    root: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct HostedUpArgs {
    #[command(flatten)]
    runtime: HostedRuntimeArgs,
    /// Bind address for the hosted HTTP API.
    #[arg(long, env = "AOS_HOSTED_BIND", default_value = DEFAULT_HOSTED_BIND)]
    bind: SocketAddr,
    /// Saved profile name to update for this hosted node.
    #[arg(long, default_value = DEFAULT_HOSTED_PROFILE)]
    profile: String,
    /// Make the hosted profile current after ensuring it exists.
    #[arg(long)]
    select: bool,
    /// Run the hosted node in the background and return after startup.
    #[arg(long)]
    background: bool,
    /// Milliseconds to wait for health before considering startup failed.
    #[arg(long, default_value_t = DEFAULT_HOSTED_STARTUP_WAIT_MS)]
    wait_ms: u64,
    /// Number of hosted worker partitions.
    #[arg(long, env = "AOS_PARTITION_COUNT", default_value_t = 1)]
    partition_count: u32,
    /// Default hosted universe used for non-routed local development.
    #[arg(long, env = "AOS_DEFAULT_UNIVERSE_ID", default_value_t = aos_node::local_universe_id())]
    default_universe_id: UniverseId,
}

#[derive(Args, Debug)]
struct HostedDownArgs {
    #[command(flatten)]
    runtime: HostedRuntimeArgs,
    /// Saved profile name to clear from current selection if it is active.
    #[arg(long, default_value = DEFAULT_HOSTED_PROFILE)]
    profile: String,
    /// Send SIGKILL after SIGTERM if the process does not exit in time.
    #[arg(long)]
    force: bool,
    /// Milliseconds to wait for process exit.
    #[arg(long, default_value_t = 5_000)]
    wait_ms: u64,
}

#[derive(Args, Debug)]
struct HostedUseArgs {
    /// Saved profile name to mark as current.
    #[arg(long, default_value = DEFAULT_HOSTED_PROFILE)]
    profile: String,
}

#[derive(Debug, Clone)]
struct HostedPaths {
    root: PathBuf,
    state_root: PathBuf,
    state: PathBuf,
    log: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HostedRuntimeState {
    pid: u32,
    api: String,
    bind: String,
    profile: String,
    state_root: PathBuf,
    log: PathBuf,
    partition_count: u32,
    default_universe_id: UniverseId,
}

#[derive(Debug, Clone, Serialize)]
struct HostedStatusView {
    root: PathBuf,
    running: bool,
    healthy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    api: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state_root: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    log: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    partition_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_universe_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct HealthInfo {
    service: String,
    version: String,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    state_root: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct DiscoveredHostedRuntime {
    state: HostedRuntimeState,
    paths: HostedPaths,
}

pub(crate) async fn handle(
    global: &GlobalOpts,
    output: OutputOpts,
    args: HostedArgs,
) -> Result<()> {
    match args.cmd {
        HostedCommand::Up(args) => handle_up(global, output, args).await,
        HostedCommand::Status(args) => handle_status(output, args).await,
        HostedCommand::Down(args) => handle_down(global, output, args).await,
        HostedCommand::Use(args) => handle_use(global, output, args),
    }
}

async fn handle_up(global: &GlobalOpts, output: OutputOpts, args: HostedUpArgs) -> Result<()> {
    let paths = resolve_hosted_paths(args.runtime.root.as_deref())?;
    fs::create_dir_all(&paths.state_root)
        .with_context(|| format!("create hosted state root {}", paths.state_root.display()))?;
    fs::create_dir_all(paths.log.parent().expect("hosted log has parent")).with_context(|| {
        format!(
            "create hosted log directory {}",
            paths.log.parent().expect("hosted log has parent").display()
        )
    })?;
    fs::create_dir_all(paths.state.parent().expect("hosted state has parent")).with_context(
        || {
            format!(
                "create hosted runtime directory {}",
                paths
                    .state
                    .parent()
                    .expect("hosted state has parent")
                    .display()
            )
        },
    )?;
    let api = format!("http://{}", args.bind);

    if let Some(existing) = load_hosted_state(&paths)? {
        let status = status_from_state(&paths, Some(&existing), 500).await?;
        if status.running && status.healthy {
            ensure_hosted_profile(
                global,
                &args.profile,
                &api,
                args.default_universe_id,
                args.select,
            )?;
            return print_success(output, serde_json::to_value(status)?, None, vec![]);
        }
        cleanup_stale_state(&paths)?;
    }

    if let Ok(health) = fetch_health(&api, 500).await {
        if health.service == "aos-node-hosted" {
            if let (Some(pid), Some(state_root)) = (health.pid, health.state_root.as_ref()) {
                if paths_equivalent(state_root, &paths.state_root) && process_is_running(pid)? {
                    let recovered = HostedRuntimeState {
                        pid,
                        api: api.clone(),
                        bind: args.bind.to_string(),
                        profile: args.profile.clone(),
                        state_root: paths.state_root.clone(),
                        log: paths.log.clone(),
                        partition_count: args.partition_count.max(1),
                        default_universe_id: args.default_universe_id,
                    };
                    save_hosted_state(&paths, &recovered)?;
                    ensure_hosted_profile(
                        global,
                        &args.profile,
                        &api,
                        args.default_universe_id,
                        args.select,
                    )?;
                    let status = status_from_state(&paths, Some(&recovered), 500).await?;
                    return print_success(output, serde_json::to_value(status)?, None, vec![]);
                }
                return Err(anyhow!(
                    "hosted bind {} is already served by another state root ({})",
                    args.bind,
                    state_root.display()
                ));
            }
            return Err(anyhow!(
                "hosted bind {} is already served by an incompatible hosted node",
                args.bind
            ));
        }
        return Err(anyhow!(
            "hosted bind {} is already served by {}",
            args.bind,
            health.service
        ));
    }

    ensure_hosted_profile(
        global,
        &args.profile,
        &api,
        args.default_universe_id,
        args.select,
    )?;

    let binary = resolve_hosted_binary()?;
    if !args.background {
        let mut child = ProcessCommand::new(&binary)
            .arg("all")
            .arg("--state-root")
            .arg(&paths.state_root)
            .arg("--bind")
            .arg(args.bind.to_string())
            .arg("--partition-count")
            .arg(args.partition_count.to_string())
            .arg("--default-universe-id")
            .arg(args.default_universe_id.to_string())
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("spawn {}", binary.display()))?;
        let state = HostedRuntimeState {
            pid: child.id(),
            api: api.clone(),
            bind: args.bind.to_string(),
            profile: args.profile.clone(),
            state_root: paths.state_root.clone(),
            log: paths.log.clone(),
            partition_count: args.partition_count.max(1),
            default_universe_id: args.default_universe_id,
        };
        save_hosted_state(&paths, &state)?;
        let status = match wait_for_healthy_status(&paths, &state, args.wait_ms).await {
            Ok(status) => status,
            Err(err) => {
                let _ = terminate_process(state.pid, true);
                let _ = cleanup_stale_state(&paths);
                return Err(err);
            }
        };
        print_success(output, serde_json::to_value(status)?, None, vec![])?;
        let exit = child.wait().context("wait for foreground hosted node")?;
        std::process::exit(exit.code().unwrap_or(1));
    }

    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log)
        .with_context(|| format!("open hosted log {}", paths.log.display()))?;
    let log_err = log
        .try_clone()
        .with_context(|| format!("clone hosted log {}", paths.log.display()))?;

    let mut command = ProcessCommand::new(&binary);
    command
        .arg("all")
        .arg("--state-root")
        .arg(&paths.state_root)
        .arg("--bind")
        .arg(args.bind.to_string())
        .arg("--partition-count")
        .arg(args.partition_count.to_string())
        .arg("--default-universe-id")
        .arg(args.default_universe_id.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    #[cfg(unix)]
    command.process_group(0);
    let child = command
        .spawn()
        .with_context(|| format!("spawn {}", binary.display()))?;

    let state = HostedRuntimeState {
        pid: child.id(),
        api: api.clone(),
        bind: args.bind.to_string(),
        profile: args.profile.clone(),
        state_root: paths.state_root.clone(),
        log: paths.log.clone(),
        partition_count: args.partition_count.max(1),
        default_universe_id: args.default_universe_id,
    };
    save_hosted_state(&paths, &state)?;

    let status = match wait_for_healthy_status(&paths, &state, args.wait_ms).await {
        Ok(status) => status,
        Err(err) => {
            let _ = terminate_process(state.pid, true);
            let _ = cleanup_stale_state(&paths);
            return Err(err);
        }
    };
    print_success(output, serde_json::to_value(status)?, None, vec![])
}

async fn handle_status(output: OutputOpts, args: HostedRuntimeArgs) -> Result<()> {
    let paths = resolve_hosted_paths(args.root.as_deref())?;
    let state = load_hosted_state(&paths)?;
    let status = status_from_state(&paths, state.as_ref(), 500).await?;
    print_success(output, serde_json::to_value(status)?, None, vec![])
}

async fn handle_down(global: &GlobalOpts, output: OutputOpts, args: HostedDownArgs) -> Result<()> {
    let paths = resolve_hosted_paths(args.runtime.root.as_deref())?;
    let discovered = if let Some(state) = load_hosted_state(&paths)? {
        Some(DiscoveredHostedRuntime {
            paths: resolve_hosted_paths_from_state_root(&state.state_root)?,
            state,
        })
    } else if args.runtime.root.is_none() {
        discover_hosted_runtime_for_down(global, &args, &paths).await?
    } else {
        None
    };
    let Some(discovered) = discovered else {
        let status = status_from_state(&paths, None, 200).await?;
        return print_success(output, serde_json::to_value(status)?, None, vec![]);
    };
    let DiscoveredHostedRuntime {
        state,
        paths: runtime_paths,
    } = discovered;

    terminate_process(state.pid, false)?;
    wait_for_process_exit(state.pid, args.wait_ms).await?;
    if args.force && process_is_running(state.pid)? {
        terminate_process(state.pid, true)?;
        wait_for_process_exit(state.pid, args.wait_ms).await?;
    }
    cleanup_stale_state(&runtime_paths)?;
    clear_current_profile_if_matches(global, &args.profile)?;
    let status = status_from_state(&runtime_paths, None, 200).await?;
    print_success(output, serde_json::to_value(status)?, None, vec![])
}

fn handle_use(global: &GlobalOpts, output: OutputOpts, args: HostedUseArgs) -> Result<()> {
    let paths = ConfigPaths::resolve(global.config.as_deref())?;
    let mut config = load_config(&paths)?;
    if !config.profiles.contains_key(&args.profile) {
        return Err(anyhow!(
            "profile '{}' not found; run `aos hosted up` first or create it explicitly",
            args.profile
        ));
    }
    config.current_profile = Some(args.profile.clone());
    save_config(&paths, &config)?;
    print_success(
        output,
        json!({
            "current_profile": args.profile,
        }),
        None,
        vec![],
    )
}

fn resolve_hosted_paths(explicit_root: Option<&Path>) -> Result<HostedPaths> {
    let root = match explicit_root {
        Some(root) => root.to_path_buf(),
        None => default_hosted_runtime_root()?,
    };
    let state_root = root.join(".aos-hosted");
    Ok(HostedPaths {
        state: state_root.join("runtime.json"),
        log: state_root.join("runtime.log"),
        root,
        state_root,
    })
}

fn default_hosted_runtime_root() -> Result<PathBuf> {
    if let Ok(root) = std::env::var("AOS_HOSTED_ROOT") {
        return Ok(PathBuf::from(root));
    }
    std::env::current_dir().context("resolve current directory for hosted runtime home")
}

fn resolve_hosted_binary() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("AOS_NODE_HOSTED_BIN") {
        return Ok(PathBuf::from(path));
    }
    let current = std::env::current_exe().context("resolve current executable")?;
    if let Some(parent) = current.parent() {
        let sibling = parent.join("aos-node-hosted");
        if sibling.exists() {
            return Ok(sibling);
        }
    }
    Ok(PathBuf::from("aos-node-hosted"))
}

fn load_hosted_state(paths: &HostedPaths) -> Result<Option<HostedRuntimeState>> {
    if !paths.state.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&paths.state)
        .with_context(|| format!("read hosted state {}", paths.state.display()))?;
    let state = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse hosted state {}", paths.state.display()))?;
    Ok(Some(state))
}

fn save_hosted_state(paths: &HostedPaths, state: &HostedRuntimeState) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(state).context("encode hosted runtime state")?;
    fs::write(&paths.state, bytes)
        .with_context(|| format!("write hosted state {}", paths.state.display()))
}

fn cleanup_stale_state(paths: &HostedPaths) -> Result<()> {
    if paths.state.exists() {
        fs::remove_file(&paths.state)
            .with_context(|| format!("remove stale hosted state {}", paths.state.display()))?;
    }
    Ok(())
}

fn resolve_hosted_paths_from_state_root(state_root: &Path) -> Result<HostedPaths> {
    let Some(root) = state_root.parent() else {
        return Err(anyhow!(
            "hosted state root '{}' has no parent runtime root",
            state_root.display()
        ));
    };
    resolve_hosted_paths(Some(root))
}

async fn discover_hosted_runtime_for_down(
    global: &GlobalOpts,
    args: &HostedDownArgs,
    fallback_paths: &HostedPaths,
) -> Result<Option<DiscoveredHostedRuntime>> {
    if let Some(runtime) =
        discover_hosted_runtime_via_profile(global, &args.profile, fallback_paths).await?
    {
        return Ok(Some(runtime));
    }
    let mut candidate_apis = Vec::new();
    if let Some(api) = global.api.as_ref() {
        push_api_candidate(&mut candidate_apis, api);
    }
    push_api_candidate(
        &mut candidate_apis,
        &format!("http://{}", DEFAULT_HOSTED_BIND),
    );
    for api in candidate_apis {
        if let Some(runtime) =
            discover_hosted_runtime_via_api(&api, &args.profile, fallback_paths, None).await?
        {
            return Ok(Some(runtime));
        }
    }
    Ok(None)
}

async fn discover_hosted_runtime_via_profile(
    global: &GlobalOpts,
    profile_name: &str,
    fallback_paths: &HostedPaths,
) -> Result<Option<DiscoveredHostedRuntime>> {
    let config_paths = ConfigPaths::resolve(global.config.as_deref())?;
    let config = load_config(&config_paths)?;
    let Some(profile) = config.profiles.get(profile_name) else {
        return Ok(None);
    };
    let default_universe_id = profile
        .universe
        .as_deref()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(aos_node::local_universe_id);
    discover_hosted_runtime_via_api(
        &profile.api,
        profile_name,
        fallback_paths,
        Some(default_universe_id),
    )
    .await
}

async fn discover_hosted_runtime_via_api(
    api: &str,
    profile_name: &str,
    fallback_paths: &HostedPaths,
    default_universe_id: Option<UniverseId>,
) -> Result<Option<DiscoveredHostedRuntime>> {
    let Ok(health) = fetch_health(api, 500).await else {
        return Ok(None);
    };
    if health.service != "aos-node-hosted" {
        return Ok(None);
    }
    let bind = bind_from_api(api).unwrap_or_else(|| {
        api.trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_end_matches('/')
            .to_string()
    });
    let pid = match health.pid {
        Some(pid) => Some(pid),
        None => pid_listening_on_bind(&bind)?,
    };
    let Some(pid) = pid else {
        return Ok(None);
    };
    if !process_is_running(pid)? {
        return Ok(None);
    }
    let runtime_paths = if let Some(state_root) = health.state_root.as_ref() {
        resolve_hosted_paths_from_state_root(state_root)?
    } else {
        fallback_paths.clone()
    };
    Ok(Some(DiscoveredHostedRuntime {
        state: HostedRuntimeState {
            pid,
            api: api.to_string(),
            bind,
            profile: profile_name.to_string(),
            state_root: health
                .state_root
                .unwrap_or_else(|| runtime_paths.state_root.clone()),
            log: runtime_paths.log.clone(),
            partition_count: 1,
            default_universe_id: default_universe_id.unwrap_or_else(aos_node::local_universe_id),
        },
        paths: runtime_paths,
    }))
}

async fn status_from_state(
    paths: &HostedPaths,
    state: Option<&HostedRuntimeState>,
    health_timeout_ms: u64,
) -> Result<HostedStatusView> {
    let pid = state.map(|state| state.pid);
    let running = if let Some(pid) = pid {
        process_is_running(pid)?
    } else {
        false
    };
    let health = if let Some(state) = state {
        fetch_health(&state.api, health_timeout_ms)
            .await
            .ok()
            .filter(|health| health_matches_runtime(health, state.pid, &state.state_root))
    } else {
        None
    };
    Ok(HostedStatusView {
        root: paths.root.clone(),
        running,
        healthy: health.is_some(),
        pid,
        api: state.map(|state| state.api.clone()),
        bind: state.map(|state| state.bind.clone()),
        profile: state.map(|state| state.profile.clone()),
        state_root: state.map(|state| state.state_root.clone()),
        log: state.map(|state| state.log.clone()),
        partition_count: state.map(|state| state.partition_count),
        default_universe_id: state.map(|state| state.default_universe_id.to_string()),
        service: health.as_ref().map(|value| value.service.clone()),
        version: health.as_ref().map(|value| value.version.clone()),
    })
}

async fn fetch_health(api: &str, timeout_ms: u64) -> Result<HealthInfo> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms.max(1)))
        .build()
        .context("build hosted status client")?;
    let response = client
        .get(format!("{}/v1/health", api.trim_end_matches('/')))
        .send()
        .await
        .with_context(|| format!("query hosted node health at {api}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("hosted node health returned {status}"));
    }
    response
        .json()
        .await
        .context("decode hosted node health response")
}

fn health_matches_runtime(health: &HealthInfo, pid: u32, state_root: &Path) -> bool {
    health.pid == Some(pid)
        && health
            .state_root
            .as_ref()
            .is_some_and(|root| paths_equivalent(root, state_root))
}

fn paths_equivalent(lhs: &Path, rhs: &Path) -> bool {
    match (lhs.canonicalize(), rhs.canonicalize()) {
        (Ok(lhs), Ok(rhs)) => lhs == rhs,
        _ => lhs == rhs,
    }
}

fn push_api_candidate(candidates: &mut Vec<String>, api: &str) {
    if !candidates.iter().any(|existing| existing == api) {
        candidates.push(api.to_string());
    }
}

fn bind_from_api(api: &str) -> Option<String> {
    let url = reqwest::Url::parse(api).ok()?;
    let host = url.host_str()?;
    let port = url.port_or_known_default()?;
    Some(format!("{host}:{port}"))
}

fn pid_listening_on_bind(bind: &str) -> Result<Option<u32>> {
    let Some((_, port)) = bind.rsplit_once(':') else {
        return Ok(None);
    };
    #[cfg(unix)]
    {
        let output = match ProcessCommand::new("lsof")
            .args(["-nP", "-tiTCP", &format!(":{port}"), "-sTCP:LISTEN"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            Ok(output) => output,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err).with_context(|| format!("inspect listener for {bind}")),
        };
        if !output.status.success() {
            return Ok(None);
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let pid = stdout
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .and_then(|line| line.parse::<u32>().ok());
        return Ok(pid);
    }
    #[cfg(not(unix))]
    {
        let _ = port;
        Ok(None)
    }
}

async fn wait_for_healthy_status(
    paths: &HostedPaths,
    state: &HostedRuntimeState,
    wait_ms: u64,
) -> Result<HostedStatusView> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms.max(1));
    loop {
        let status = status_from_state(paths, Some(state), 500).await?;
        if status.healthy {
            return Ok(status);
        }
        if let Some(reason) = health_incompatibility_reason(state).await? {
            return Err(anyhow!("{reason}"));
        }
        if !status.running {
            return Err(anyhow!(
                "hosted node process {} exited before becoming healthy",
                state.pid
            ));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "hosted node did not become healthy within {} ms; retry with --wait-ms if cold start needs more time",
                wait_ms.max(1)
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn health_incompatibility_reason(state: &HostedRuntimeState) -> Result<Option<String>> {
    let Ok(health) = fetch_health(&state.api, 500).await else {
        return Ok(None);
    };
    if health.service != "aos-node-hosted" {
        return Ok(Some(format!(
            "hosted bind {} is served by {} instead of aos-node-hosted",
            state.bind, health.service
        )));
    }
    let Some(pid) = health.pid else {
        return Ok(Some(format!(
            "hosted node at {} responded to /v1/health without pid/state_root; this usually means `aos-node-hosted` is older than `aos`. Rebuild it with `cargo build -p aos-node-hosted --bin aos-node-hosted --features legacy-bins`",
            state.api
        )));
    };
    let Some(state_root) = health.state_root.as_ref() else {
        return Ok(Some(format!(
            "hosted node at {} responded to /v1/health without pid/state_root; this usually means `aos-node-hosted` is older than `aos`. Rebuild it with `cargo build -p aos-node-hosted --bin aos-node-hosted --features legacy-bins`",
            state.api
        )));
    };
    if pid != state.pid || !paths_equivalent(state_root, &state.state_root) {
        return Ok(Some(format!(
            "hosted bind {} is served by another hosted runtime (pid {}, state root {})",
            state.bind,
            pid,
            state_root.display()
        )));
    }
    Ok(None)
}

fn ensure_hosted_profile(
    global: &GlobalOpts,
    profile_name: &str,
    api: &str,
    default_universe_id: UniverseId,
    select: bool,
) -> Result<()> {
    let paths = ConfigPaths::resolve(global.config.as_deref())?;
    let mut config = load_config(&paths)?;
    let profile = config
        .profiles
        .entry(profile_name.to_string())
        .or_insert_with(|| ProfileConfig {
            kind: ProfileKind::Remote,
            api: api.to_string(),
            token: None,
            token_env: None,
            headers: Default::default(),
            universe: Some(default_universe_id.to_string()),
            world: None,
        });
    profile.kind = ProfileKind::Remote;
    profile.api = api.to_string();
    profile.token = None;
    profile.token_env = None;
    profile.universe = Some(default_universe_id.to_string());
    if select || config.current_profile.is_none() {
        config.current_profile = Some(profile_name.to_string());
    }
    save_config(&paths, &config)
}

fn clear_current_profile_if_matches(global: &GlobalOpts, profile_name: &str) -> Result<()> {
    let paths = ConfigPaths::resolve(global.config.as_deref())?;
    let mut config = load_config(&paths)?;
    if config.current_profile.as_deref() == Some(profile_name) {
        config.current_profile = None;
        save_config(&paths, &config)?;
    }
    Ok(())
}

fn process_is_running(pid: u32) -> Result<bool> {
    #[cfg(unix)]
    {
        let ps = ProcessCommand::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .with_context(|| format!("inspect process {pid}"))?;
        if !ps.status.success() {
            return Ok(false);
        }
        let stat = String::from_utf8_lossy(&ps.stdout);
        let stat = stat.trim();
        if stat.is_empty() {
            return Ok(false);
        }
        if stat.starts_with('Z') {
            return Ok(false);
        }
        let status = ProcessCommand::new("kill")
            .args(["-0", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .with_context(|| format!("check process {pid}"))?;
        Ok(status.success())
    }
    #[cfg(windows)]
    {
        let output = ProcessCommand::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}")])
            .output()
            .with_context(|| format!("check process {pid}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.contains(&pid.to_string()))
    }
}

fn terminate_process(pid: u32, force: bool) -> Result<()> {
    #[cfg(unix)]
    {
        let signal = if force { "KILL" } else { "TERM" };
        let status = ProcessCommand::new("kill")
            .args(["-s", signal, &pid.to_string()])
            .status()
            .with_context(|| format!("send {signal} to process {pid}"))?;
        if !status.success() {
            return Err(anyhow!("failed to send {signal} to process {pid}"));
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        let mut command = ProcessCommand::new("taskkill");
        command.args(["/PID", &pid.to_string()]);
        if force {
            command.arg("/F");
        }
        let status = command
            .status()
            .with_context(|| format!("terminate process {pid}"))?;
        if !status.success() {
            return Err(anyhow!("failed to terminate process {pid}"));
        }
        Ok(())
    }
}

async fn wait_for_process_exit(pid: u32, wait_ms: u64) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        if !process_is_running(pid)? {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow!("process {pid} did not exit before timeout"));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct TestCli {
        #[command(flatten)]
        args: HostedArgs,
    }

    #[test]
    fn hosted_up_default_wait_budget_covers_cold_start_replay() {
        let cli = TestCli::parse_from(["test", "up"]);
        let HostedCommand::Up(args) = cli.args.cmd else {
            panic!("expected hosted up command");
        };
        assert_eq!(args.wait_ms, DEFAULT_HOSTED_STARTUP_WAIT_MS);
    }

    #[test]
    fn bind_from_api_extracts_socket_address() {
        assert_eq!(
            bind_from_api("http://127.0.0.1:9011"),
            Some("127.0.0.1:9011".to_string())
        );
        assert_eq!(
            bind_from_api("https://example.test"),
            Some("example.test:443".to_string())
        );
    }
}
