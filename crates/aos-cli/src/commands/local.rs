use std::fs;
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use aos_sqlite::LocalStatePaths;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::GlobalOpts;
use crate::config::{ConfigPaths, ProfileConfig, ProfileKind, load_config, save_config};
use crate::output::{OutputOpts, print_success};

const DEFAULT_LOCAL_PROFILE: &str = "local";
const DEFAULT_LOCAL_UNIVERSE: &str = "local";
const DEFAULT_LOCAL_BIND: &str = "127.0.0.1:9080";

#[derive(Args, Debug)]
#[command(about = "Manage the local AgentOS node")]
pub(crate) struct LocalArgs {
    #[command(subcommand)]
    cmd: LocalCommand,
}

#[derive(Subcommand, Debug)]
enum LocalCommand {
    /// Start the local node and ensure the local CLI profile points at it.
    Up(LocalUpArgs),
    /// Show local node process and health status.
    Status(LocalRuntimeArgs),
    /// Stop the local node process managed from this runtime home.
    Down(LocalDownArgs),
    /// Select the reserved local profile as the current CLI profile.
    Use(LocalUseArgs),
}

#[derive(Args, Debug, Clone)]
struct LocalRuntimeArgs {
    /// Override the local runtime home directory.
    #[arg(long, env = "AOS_LOCAL_ROOT")]
    root: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct LocalUpArgs {
    #[command(flatten)]
    runtime: LocalRuntimeArgs,
    /// Bind address for the local HTTP API.
    #[arg(long, env = "AOS_LOCAL_BIND", default_value = DEFAULT_LOCAL_BIND)]
    bind: SocketAddr,
    /// Saved profile name to update for this local node.
    #[arg(long, default_value = DEFAULT_LOCAL_PROFILE)]
    profile: String,
    /// Make the local profile current after ensuring it exists.
    #[arg(long)]
    select: bool,
    /// Run the local node in the background and return after startup.
    #[arg(long)]
    background: bool,
    /// Milliseconds to wait for health before considering startup failed.
    #[arg(long, default_value_t = 10_000)]
    wait_ms: u64,
}

#[derive(Args, Debug)]
struct LocalDownArgs {
    #[command(flatten)]
    runtime: LocalRuntimeArgs,
    /// Saved profile name to clear from current selection if it is active.
    #[arg(long, default_value = DEFAULT_LOCAL_PROFILE)]
    profile: String,
    /// Send SIGKILL after SIGTERM if the process does not exit in time.
    #[arg(long)]
    force: bool,
    /// Milliseconds to wait for process exit.
    #[arg(long, default_value_t = 5_000)]
    wait_ms: u64,
}

#[derive(Args, Debug)]
struct LocalUseArgs {
    /// Saved profile name to mark as current.
    #[arg(long, default_value = DEFAULT_LOCAL_PROFILE)]
    profile: String,
}

#[derive(Debug, Clone)]
struct LocalPaths {
    root: PathBuf,
    state_root: LocalStatePaths,
    state: PathBuf,
    log: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalRuntimeState {
    pid: u32,
    api: String,
    bind: String,
    profile: String,
    state_root: PathBuf,
    log: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
struct LocalStatusView {
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
    service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

pub(crate) async fn handle(global: &GlobalOpts, output: OutputOpts, args: LocalArgs) -> Result<()> {
    match args.cmd {
        LocalCommand::Up(args) => handle_up(global, output, args).await,
        LocalCommand::Status(args) => handle_status(output, args).await,
        LocalCommand::Down(args) => handle_down(global, output, args).await,
        LocalCommand::Use(args) => handle_use(global, output, args),
    }
}

async fn handle_up(global: &GlobalOpts, output: OutputOpts, args: LocalUpArgs) -> Result<()> {
    let paths = resolve_local_paths(args.runtime.root.as_deref())?;
    paths.state_root.ensure_root().with_context(|| {
        format!(
            "create local state root {}",
            paths.state_root.root().display()
        )
    })?;
    fs::create_dir_all(paths.log.parent().expect("local log has parent")).with_context(|| {
        format!(
            "create local log directory {}",
            paths.log.parent().expect("local log has parent").display()
        )
    })?;
    fs::create_dir_all(paths.state.parent().expect("local state has parent")).with_context(
        || {
            format!(
                "create local runtime directory {}",
                paths
                    .state
                    .parent()
                    .expect("local state has parent")
                    .display()
            )
        },
    )?;
    let api = format!("http://{}", args.bind);

    if let Some(existing) = load_local_state(&paths)? {
        let status = status_from_state(&paths, Some(&existing), 500).await?;
        if status.running && status.healthy {
            ensure_local_profile(
                global,
                &args.profile,
                &api,
                DEFAULT_LOCAL_UNIVERSE,
                args.select,
            )?;
            return print_success(output, serde_json::to_value(status)?, None, vec![]);
        }
        cleanup_stale_state(&paths)?;
    }

    ensure_local_profile(
        global,
        &args.profile,
        &api,
        DEFAULT_LOCAL_UNIVERSE,
        args.select,
    )?;

    let binary = resolve_local_binary()?;
    if !args.background {
        let mut child = ProcessCommand::new(&binary)
            .arg("serve")
            .arg("--state-root")
            .arg(paths.state_root.root())
            .arg("--bind")
            .arg(args.bind.to_string())
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("spawn {}", binary.display()))?;
        let state = LocalRuntimeState {
            pid: child.id(),
            api: api.clone(),
            bind: args.bind.to_string(),
            profile: args.profile.clone(),
            state_root: paths.state_root.root().to_path_buf(),
            log: paths.log.clone(),
        };
        save_local_state(&paths, &state)?;
        let status = status_from_state(&paths, Some(&state), args.wait_ms).await?;
        print_success(output, serde_json::to_value(status)?, None, vec![])?;
        let exit = child.wait().context("wait for foreground local node")?;
        std::process::exit(exit.code().unwrap_or(1));
    }

    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log)
        .with_context(|| format!("open local log {}", paths.log.display()))?;
    let log_err = log
        .try_clone()
        .with_context(|| format!("clone local log {}", paths.log.display()))?;

    let mut command = ProcessCommand::new(&binary);
    command
        .arg("serve")
        .arg("--state-root")
        .arg(paths.state_root.root())
        .arg("--bind")
        .arg(args.bind.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    #[cfg(unix)]
    command.process_group(0);
    let child = command
        .spawn()
        .with_context(|| format!("spawn {}", binary.display()))?;

    let state = LocalRuntimeState {
        pid: child.id(),
        api: api.clone(),
        bind: args.bind.to_string(),
        profile: args.profile.clone(),
        state_root: paths.state_root.root().to_path_buf(),
        log: paths.log.clone(),
    };
    save_local_state(&paths, &state)?;

    let status = match status_from_state(&paths, Some(&state), args.wait_ms).await {
        Ok(status) => status,
        Err(err) => {
            let _ = terminate_process(state.pid, true);
            let _ = cleanup_stale_state(&paths);
            return Err(err);
        }
    };
    print_success(output, serde_json::to_value(status)?, None, vec![])
}

async fn handle_status(output: OutputOpts, args: LocalRuntimeArgs) -> Result<()> {
    let paths = resolve_local_paths(args.root.as_deref())?;
    let state = load_local_state(&paths)?;
    let status = status_from_state(&paths, state.as_ref(), 500).await?;
    print_success(output, serde_json::to_value(status)?, None, vec![])
}

async fn handle_down(global: &GlobalOpts, output: OutputOpts, args: LocalDownArgs) -> Result<()> {
    let paths = resolve_local_paths(args.runtime.root.as_deref())?;
    let state = load_local_state(&paths)?;
    let Some(state) = state else {
        let status = status_from_state(&paths, None, 200).await?;
        return print_success(output, serde_json::to_value(status)?, None, vec![]);
    };

    terminate_process(state.pid, false)?;
    wait_for_process_exit(state.pid, args.wait_ms).await?;
    if args.force && process_is_running(state.pid)? {
        terminate_process(state.pid, true)?;
        wait_for_process_exit(state.pid, args.wait_ms).await?;
    }
    cleanup_stale_state(&paths)?;
    clear_current_profile_if_matches(global, &args.profile)?;
    let status = status_from_state(&paths, None, 200).await?;
    print_success(output, serde_json::to_value(status)?, None, vec![])
}

fn handle_use(global: &GlobalOpts, output: OutputOpts, args: LocalUseArgs) -> Result<()> {
    let paths = ConfigPaths::resolve(global.config.as_deref())?;
    let mut config = load_config(&paths)?;
    if !config.profiles.contains_key(&args.profile) {
        return Err(anyhow!(
            "profile '{}' not found; run `aos local up` first or create it explicitly",
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

fn resolve_local_paths(explicit_root: Option<&Path>) -> Result<LocalPaths> {
    let root = match explicit_root {
        Some(root) => root.to_path_buf(),
        None => default_local_runtime_root()?,
    };
    let state_root = LocalStatePaths::new(root.join(".aos"));
    Ok(LocalPaths {
        state: state_root.runtime_state_file(),
        log: state_root.runtime_log_file(),
        root,
        state_root,
    })
}

fn default_local_runtime_root() -> Result<PathBuf> {
    if let Ok(root) = std::env::var("AOS_LOCAL_ROOT") {
        return Ok(PathBuf::from(root));
    }
    if let Ok(root) = std::env::var("XDG_STATE_HOME") {
        return Ok(PathBuf::from(root).join("aos/local"));
    }
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home).join(".local/state/aos/local"));
    }
    Ok(std::env::current_dir()
        .context("resolve current directory for local runtime home")?
        .join(".aos/local"))
}

fn resolve_local_binary() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("AOS_NODE_LOCAL_BIN") {
        return Ok(PathBuf::from(path));
    }
    let current = std::env::current_exe().context("resolve current executable")?;
    if let Some(parent) = current.parent() {
        let sibling = parent.join("aos-node-local");
        if sibling.exists() {
            return Ok(sibling);
        }
    }
    Ok(PathBuf::from("aos-node-local"))
}

fn load_local_state(paths: &LocalPaths) -> Result<Option<LocalRuntimeState>> {
    if !paths.state.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&paths.state)
        .with_context(|| format!("read local state {}", paths.state.display()))?;
    let state = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse local state {}", paths.state.display()))?;
    Ok(Some(state))
}

fn save_local_state(paths: &LocalPaths, state: &LocalRuntimeState) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(state).context("encode local runtime state")?;
    fs::write(&paths.state, bytes)
        .with_context(|| format!("write local state {}", paths.state.display()))
}

fn cleanup_stale_state(paths: &LocalPaths) -> Result<()> {
    if paths.state.exists() {
        fs::remove_file(&paths.state)
            .with_context(|| format!("remove stale local state {}", paths.state.display()))?;
    }
    Ok(())
}

async fn status_from_state(
    paths: &LocalPaths,
    state: Option<&LocalRuntimeState>,
    health_timeout_ms: u64,
) -> Result<LocalStatusView> {
    let pid = state.map(|state| state.pid);
    let running = if let Some(pid) = pid {
        process_is_running(pid)?
    } else {
        false
    };
    let health = if let Some(state) = state {
        fetch_health(&state.api, health_timeout_ms).await.ok()
    } else {
        None
    };
    Ok(LocalStatusView {
        root: paths.root.clone(),
        running,
        healthy: health.is_some(),
        pid,
        api: state.map(|state| state.api.clone()),
        bind: state.map(|state| state.bind.clone()),
        profile: state.map(|state| state.profile.clone()),
        state_root: state.map(|state| state.state_root.clone()),
        log: state.map(|state| state.log.clone()),
        service: health
            .as_ref()
            .and_then(|value| value.get("service"))
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
        version: health
            .as_ref()
            .and_then(|value| value.get("version"))
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
    })
}

async fn fetch_health(api: &str, timeout_ms: u64) -> Result<serde_json::Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms.max(1)))
        .build()
        .context("build local status client")?;
    let response = client
        .get(format!("{}/v1/health", api.trim_end_matches('/')))
        .send()
        .await
        .with_context(|| format!("query local node health at {api}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("local node health returned {status}"));
    }
    response
        .json()
        .await
        .context("decode local node health response")
}

fn ensure_local_profile(
    global: &GlobalOpts,
    profile_name: &str,
    api: &str,
    universe: &str,
    select: bool,
) -> Result<()> {
    let paths = ConfigPaths::resolve(global.config.as_deref())?;
    let mut config = load_config(&paths)?;
    let profile = config
        .profiles
        .entry(profile_name.to_string())
        .or_insert_with(|| ProfileConfig {
            kind: ProfileKind::Local,
            api: api.to_string(),
            token: None,
            token_env: None,
            headers: Default::default(),
            universe: Some(universe.to_string()),
            world: None,
        });
    profile.kind = ProfileKind::Local;
    profile.api = api.to_string();
    profile.token = None;
    profile.token_env = None;
    profile.universe = Some(universe.to_string());
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
        let status = ProcessCommand::new("kill")
            .args(["-0", &pid.to_string()])
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
