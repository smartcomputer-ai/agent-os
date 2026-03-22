use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use serde_json::Value as JsonValue;

use aos_node_hosted::load_dotenv_candidates;

const HEALTH_URL: &str = "http://127.0.0.1:9011/v1/health";
const ACTIVATION_FAILURE_NEEDLE: &str = "disabling hosted world after activation error";

#[derive(Parser, Debug)]
#[command(name = "hosted-repro")]
#[command(about = "Managed harness for the hosted Demiurge restart corruption repro.")]
struct Args {
    #[arg(long, default_value = "target/debug/aos-node-hosted")]
    node_bin: PathBuf,

    #[arg(long, default_value = "target/debug/aos")]
    aos_bin: PathBuf,

    #[arg(long, default_value = "worlds/demiurge/scripts/demiurge_task.sh")]
    task_script: PathBuf,

    #[arg(long, default_value = "worlds/demiurge")]
    world_root: PathBuf,

    #[arg(long)]
    state_root: Option<PathBuf>,

    #[arg(long)]
    log_file: Option<PathBuf>,

    #[arg(long, default_value_t = 2)]
    task_count: usize,

    #[arg(long, default_value = "Echo YO-YOO.")]
    task_text: String,

    #[arg(long, default_value_t = 5_000)]
    startup_wait_ms: u64,

    #[arg(long, default_value_t = 5_000)]
    post_restart_wait_ms: u64,

    #[arg(long)]
    no_reset: bool,

    #[arg(long)]
    keep_node_running: bool,
}

struct ManagedNode {
    child: Child,
}

impl ManagedNode {
    fn stop(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }
        self.child.kill().context("kill aos-node-hosted")?;
        let _ = self.child.wait();
        Ok(())
    }
}

impl Drop for ManagedNode {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn main() -> Result<()> {
    load_dotenv_candidates()?;
    let args = Args::parse();

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .context("canonicalize repo root")?;
    let state_root = args
        .state_root
        .clone()
        .unwrap_or_else(|| repo_root.join(".aos-hosted"));
    let log_file = args
        .log_file
        .clone()
        .unwrap_or_else(|| state_root.join("repro-node.log"));

    if !args.no_reset {
        reset_environment(&repo_root, &args.aos_bin, &state_root)?;
    }

    run_repro(&repo_root, &args, &state_root, &log_file)
}

fn run_repro(repo_root: &Path, args: &Args, state_root: &Path, log_file: &Path) -> Result<()> {
    let mut node = start_node(args, repo_root, state_root, log_file, "initial start")?;
    run_aos(&args.aos_bin, repo_root, &["hosted", "use"])?;
    let create_output = run_aos_capture(
        &args.aos_bin,
        repo_root,
        &[
            "world",
            "create",
            "--local-root",
            &args.world_root.display().to_string(),
            "--sync-secrets",
            "--select",
            "--json",
        ],
    )?;
    let create_doc: JsonValue =
        serde_json::from_str(&create_output).context("parse world create json")?;
    let world_id = create_doc
        .get("data")
        .and_then(|data| data.get("world_id"))
        .or_else(|| create_doc.get("world_id"))
        .and_then(JsonValue::as_str)
        .ok_or_else(|| anyhow!("world create output missing world_id"))?
        .to_owned();
    wait_for_world_visible(&args.aos_bin, repo_root, &world_id, args.startup_wait_ms)?;

    restart_node(
        &mut node,
        args,
        repo_root,
        state_root,
        log_file,
        "clean restart before tasks",
    )?;
    wait_for_world_visible(&args.aos_bin, repo_root, &world_id, args.startup_wait_ms)?;

    for task_index in 1..=args.task_count {
        eprintln!("running task {task_index}/{}...", args.task_count);
        let task_text = args.task_text.clone();
        run_script(
            &args.task_script,
            repo_root,
            &["--task", task_text.as_str()],
        )?;
    }

    restart_node(
        &mut node,
        args,
        repo_root,
        state_root,
        log_file,
        "restart after tasks",
    )?;
    wait_for_world_visible(&args.aos_bin, repo_root, &world_id, args.startup_wait_ms)?;
    thread::sleep(Duration::from_millis(args.post_restart_wait_ms));

    let log_text = fs::read_to_string(log_file)
        .with_context(|| format!("read node log {}", log_file.display()))?;
    if let Some(line) = log_text
        .lines()
        .find(|line| line.contains(ACTIVATION_FAILURE_NEEDLE))
    {
        println!("reproduced=true");
        println!("world_id={world_id}");
        println!("log_file={}", log_file.display());
        println!("failure_line={line}");
        if !args.keep_node_running {
            node.stop()?;
        }
        return Ok(());
    }

    let world_get = run_aos_capture(
        &args.aos_bin,
        repo_root,
        &["world", "get", "--json", "--world", &world_id],
    )
    .unwrap_or_else(|err| format!("world_get_error={err:#}"));
    let trace_summary = run_aos_capture(
        &args.aos_bin,
        repo_root,
        &["world", "trace", "summary", "--json", "--world", &world_id],
    )
    .unwrap_or_else(|err| format!("trace_summary_error={err:#}"));

    if !args.keep_node_running {
        node.stop()?;
    }
    bail!(
        "did not reproduce activation corruption for world {}\nlog_file={}\nworld_get={}\ntrace_summary={}",
        world_id,
        log_file.display(),
        world_get.trim(),
        trace_summary.trim()
    );
}

fn wait_for_world_visible(
    aos_bin: &Path,
    repo_root: &Path,
    world_id: &str,
    timeout_ms: u64,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if run_aos_capture(
            aos_bin,
            repo_root,
            &["world", "get", "--json", "--world", world_id],
        )
        .is_ok()
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!("timed out waiting for world {} to become visible", world_id);
}

fn reset_environment(repo_root: &Path, aos_bin: &Path, state_root: &Path) -> Result<()> {
    let _ = run_aos_capture(aos_bin, repo_root, &["hosted", "down", "--json"]);
    run_script(
        &repo_root.join("dev/scripts/hosted-topics-reset.sh"),
        repo_root,
        &[],
    )?;
    let _ = fs::remove_dir_all(state_root);
    fs::create_dir_all(state_root)
        .with_context(|| format!("create state root {}", state_root.display()))?;
    Ok(())
}

fn restart_node(
    node: &mut ManagedNode,
    args: &Args,
    repo_root: &Path,
    state_root: &Path,
    log_file: &Path,
    label: &str,
) -> Result<()> {
    node.stop()?;
    *node = start_node(args, repo_root, state_root, log_file, label)?;
    Ok(())
}

fn start_node(
    args: &Args,
    repo_root: &Path,
    state_root: &Path,
    log_file: &Path,
    label: &str,
) -> Result<ManagedNode> {
    fs::create_dir_all(state_root)
        .with_context(|| format!("create state root {}", state_root.display()))?;
    let mut marker = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
        .with_context(|| format!("open log file {}", log_file.display()))?;
    writeln!(marker, "\n=== {} @ {:?} ===", label, Instant::now()).context("write log marker")?;
    drop(marker);

    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
        .with_context(|| format!("open stdout log {}", log_file.display()))?;
    let stderr = stdout
        .try_clone()
        .with_context(|| format!("clone stderr log {}", log_file.display()))?;

    let mut command = Command::new(&args.node_bin);
    command
        .current_dir(repo_root)
        .arg("--state-root")
        .arg(state_root)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let child = command
        .spawn()
        .with_context(|| format!("spawn {}", args.node_bin.display()))?;

    wait_for_health(args.startup_wait_ms, repo_root)?;
    Ok(ManagedNode { child })
}

fn wait_for_health(startup_wait_ms: u64, repo_root: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_millis(startup_wait_ms);
    while Instant::now() < deadline {
        let status = Command::new("curl")
            .current_dir(repo_root)
            .args(["-sf", HEALTH_URL])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if matches!(status, Ok(status) if status.success()) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!("timed out waiting for {}", HEALTH_URL);
}

fn run_aos(aos_bin: &Path, repo_root: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new(aos_bin)
        .current_dir(repo_root)
        .args(args)
        .output()
        .with_context(|| format!("run {}", render_command(aos_bin, args)))?;
    if output.status.success() {
        return Ok(());
    }
    Err(anyhow!(
        "{} failed:\nstdout:\n{}\nstderr:\n{}",
        render_command(aos_bin, args),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ))
}

fn run_aos_capture(aos_bin: &Path, repo_root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new(aos_bin)
        .current_dir(repo_root)
        .args(args)
        .output()
        .with_context(|| format!("run {}", render_command(aos_bin, args)))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    Err(anyhow!(
        "{} failed:\nstdout:\n{}\nstderr:\n{}",
        render_command(aos_bin, args),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ))
}

fn run_script(script: &Path, repo_root: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new(script)
        .current_dir(repo_root)
        .args(args)
        .output()
        .with_context(|| format!("run {}", render_command(script, args)))?;
    if output.status.success() {
        return Ok(());
    }
    Err(anyhow!(
        "{} failed:\nstdout:\n{}\nstderr:\n{}",
        render_command(script, args),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ))
}

fn render_command(program: &Path, args: &[&str]) -> String {
    let mut rendered = program.display().to_string();
    for arg in args {
        rendered.push(' ');
        rendered.push_str(arg);
    }
    rendered
}
