mod agent_session;
mod aggregator;
mod blob_echo;
mod chain_comp;
mod counter;
mod example_host;
mod fetch_notify;
mod hello_timer;
mod llm_summarizer;
mod retry_backoff;
mod safe_upgrade;
mod trace_failure_classification;
mod util;
mod workspaces;

use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use once_cell::sync::Lazy;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Once;

#[derive(Parser, Debug)]
#[command(name = "aos-smoke", version, about = "Run AgentOS ladder demos")]
struct Cli {
    #[arg(
        long,
        default_value_t = false,
        help = "Force recompilation of reducers, bypassing cache"
    )]
    force_build: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the Counter state machine example
    Counter,
    /// Run the Hello Timer micro-effect example
    HelloTimer,
    /// Run the Blob Echo micro-effect example
    BlobEcho,
    /// Run the Fetch & Notify plan example
    FetchNotify,
    /// Run the Aggregator fan-out example
    Aggregator,
    /// Run the Chain + Compensation example
    ChainComp,
    /// Run the governance safe-upgrade example
    SafeUpgrade,
    /// Run the LLM summarizer example
    LlmSummarizer,
    /// Run the reducer-driven retry with backoff example
    RetryBackoff,
    /// Run the workspace plan + caps example
    Workspaces,
    /// Run the agent session lifecycle example
    AgentSession,
    /// Run the trace failure-classification example
    TraceFailureClassification,
    /// Run core fixtures (00-19) sequentially
    All,
    /// Run Agent SDK fixtures (20+) sequentially
    AllAgent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExampleGroup {
    Core,
    Agent,
}

#[derive(Debug)]
struct ExampleMeta {
    number: &'static str,
    slug: &'static str,
    title: &'static str,
    summary: &'static str,
    group: ExampleGroup,
    dir: &'static str,
    runner: fn(&Path) -> Result<()>,
}

const EXAMPLES: &[ExampleMeta] = &[
    ExampleMeta {
        number: "00",
        slug: "counter",
        title: "CounterSM",
        summary: "Reducer typestate without effects",
        group: ExampleGroup::Core,
        dir: "crates/aos-smoke/fixtures/00-counter",
        runner: counter::run,
    },
    ExampleMeta {
        number: "01",
        slug: "hello-timer",
        title: "Hello Timer",
        summary: "Reducer micro-effect timer demo",
        group: ExampleGroup::Core,
        dir: "crates/aos-smoke/fixtures/01-hello-timer",
        runner: hello_timer::run,
    },
    ExampleMeta {
        number: "02",
        slug: "blob-echo",
        title: "Blob Echo",
        summary: "Reducer blob.put/get demo",
        group: ExampleGroup::Core,
        dir: "crates/aos-smoke/fixtures/02-blob-echo",
        runner: blob_echo::run,
    },
    ExampleMeta {
        number: "03",
        slug: "fetch-notify",
        title: "Fetch & Notify",
        summary: "Plan-triggered HTTP orchestration",
        group: ExampleGroup::Core,
        dir: "crates/aos-smoke/fixtures/03-fetch-notify",
        runner: fetch_notify::run,
    },
    ExampleMeta {
        number: "04",
        slug: "aggregator",
        title: "Aggregator",
        summary: "Fan-out plan with http receipts",
        group: ExampleGroup::Core,
        dir: "crates/aos-smoke/fixtures/04-aggregator",
        runner: aggregator::run,
    },
    ExampleMeta {
        number: "05",
        slug: "chain-comp",
        title: "Chain + Compensation",
        summary: "Multi-plan saga w/ refund path",
        group: ExampleGroup::Core,
        dir: "crates/aos-smoke/fixtures/05-chain-comp",
        runner: chain_comp::run,
    },
    ExampleMeta {
        number: "06",
        slug: "safe-upgrade",
        title: "Safe Upgrade",
        summary: "Governance shadow/apply demo",
        group: ExampleGroup::Core,
        dir: "crates/aos-smoke/fixtures/06-safe-upgrade",
        runner: safe_upgrade::run,
    },
    ExampleMeta {
        number: "07",
        slug: "llm-summarizer",
        title: "LLM Summarizer",
        summary: "HTTP + LLM with mocked receipt",
        group: ExampleGroup::Core,
        dir: "crates/aos-smoke/fixtures/07-llm-summarizer",
        runner: llm_summarizer::run,
    },
    ExampleMeta {
        number: "08",
        slug: "retry-backoff",
        title: "Retry Backoff",
        summary: "Reducer-driven retries w/ timer.set",
        group: ExampleGroup::Core,
        dir: "crates/aos-smoke/fixtures/08-retry-backoff",
        runner: retry_backoff::run,
    },
    ExampleMeta {
        number: "09",
        slug: "workspaces",
        title: "Workspaces",
        summary: "Workspace plan + caps demo",
        group: ExampleGroup::Core,
        dir: "crates/aos-smoke/fixtures/09-workspaces",
        runner: workspaces::run,
    },
    ExampleMeta {
        number: "10",
        slug: "trace-failure-classification",
        title: "Trace Failure",
        summary: "Trace-get/diagnose failure classification",
        group: ExampleGroup::Core,
        dir: "crates/aos-smoke/fixtures/10-trace-failure-classification",
        runner: trace_failure_classification::run,
    },
    ExampleMeta {
        number: "20",
        slug: "agent-session",
        title: "Agent Session",
        summary: "SDK session run-start + replay parity",
        group: ExampleGroup::Agent,
        dir: "crates/aos-smoke/fixtures/20-agent-session",
        runner: agent_session::run,
    },
];

fn main() {
    init_logging();
    if let Err(err) = run_cli() {
        eprintln!("error: {err}");
        for cause in err.chain().skip(1) {
            eprintln!("  caused by: {cause}");
        }
        process::exit(1);
    }
}

fn init_logging() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .format_timestamp_millis()
            .try_init();
    });
}

fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    util::set_force_build(cli.force_build);
    match cli.command {
        Some(Commands::Counter) => run_single("counter"),
        Some(Commands::HelloTimer) => run_single("hello-timer"),
        Some(Commands::BlobEcho) => run_single("blob-echo"),
        Some(Commands::FetchNotify) => run_single("fetch-notify"),
        Some(Commands::Aggregator) => run_single("aggregator"),
        Some(Commands::ChainComp) => run_single("chain-comp"),
        Some(Commands::SafeUpgrade) => run_single("safe-upgrade"),
        Some(Commands::LlmSummarizer) => run_single("llm-summarizer"),
        Some(Commands::RetryBackoff) => run_single("retry-backoff"),
        Some(Commands::Workspaces) => run_single("workspaces"),
        Some(Commands::AgentSession) => run_single("agent-session"),
        Some(Commands::TraceFailureClassification) => run_single("trace-failure-classification"),
        Some(Commands::All) => run_group(ExampleGroup::Core),
        Some(Commands::AllAgent) => run_group(ExampleGroup::Agent),
        None => {
            list_examples();
            Ok(())
        }
    }
}

fn list_examples() {
    println!("Core fixtures (00-19):\n");
    list_examples_for_group(ExampleGroup::Core);
    println!("\nAgent SDK fixtures (20+):\n");
    list_examples_for_group(ExampleGroup::Agent);
    println!("\nGroup commands:");
    println!("  all       Run core fixtures (00-19)");
    println!("  all-agent Run Agent SDK fixtures (20+)");
}

fn list_examples_for_group(group: ExampleGroup) {
    for ex in EXAMPLES.iter().filter(|ex| ex.group == group) {
        println!(
            "{:<3} {:<12} {:<16} {}",
            ex.number, ex.slug, ex.title, ex.summary
        );
    }
}

fn run_single(slug: &str) -> Result<()> {
    let ex = EXAMPLES
        .iter()
        .find(|ex| ex.slug == slug)
        .ok_or_else(|| anyhow!("unknown example '{slug}'"))?;
    let abs_dir = example_root(ex);
    ensure_structure_exists(&abs_dir)?;
    println!(
        "Running example {number} — {title} ({slug})",
        number = ex.number,
        title = ex.title,
        slug = ex.slug
    );
    (ex.runner)(&abs_dir)
}

fn run_group(group: ExampleGroup) -> Result<()> {
    for ex in EXAMPLES.iter().filter(|ex| ex.group == group) {
        run_single(ex.slug)?;
    }
    Ok(())
}

fn ensure_structure_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("missing directory '{}'", path.display()));
    }
    Ok(())
}

fn example_root(meta: &ExampleMeta) -> PathBuf {
    WORKSPACE_ROOT.join(meta.dir)
}

static WORKSPACE_ROOT: Lazy<PathBuf> = Lazy::new(|| {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("../ from crate")
        .parent()
        .expect("workspace root")
        .to_path_buf()
});

pub(crate) fn workspace_root() -> &'static Path {
    &WORKSPACE_ROOT
}
