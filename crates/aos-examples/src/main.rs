mod examples;

use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use once_cell::sync::Lazy;
use std::path::{Path, PathBuf};
use std::process;

#[derive(Parser, Debug)]
#[command(name = "aos-examples", version, about = "Run AgentOS ladder demos")]
struct Cli {
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
    /// Run every available example sequentially
    All,
}

#[derive(Debug)]
struct ExampleMeta {
    number: &'static str,
    slug: &'static str,
    title: &'static str,
    summary: &'static str,
    dir: &'static str,
}

const EXAMPLES: &[ExampleMeta] = &[
    ExampleMeta {
        number: "00",
        slug: "counter",
        title: "CounterSM",
        summary: "Reducer typestate without effects",
        dir: "examples/00-counter",
    },
    ExampleMeta {
        number: "01",
        slug: "hello-timer",
        title: "Hello Timer",
        summary: "Reducer micro-effect timer demo",
        dir: "examples/01-hello-timer",
    },
    ExampleMeta {
        number: "02",
        slug: "blob-echo",
        title: "Blob Echo",
        summary: "Reducer blob.put/get demo",
        dir: "examples/02-blob-echo",
    },
];

fn main() {
    if let Err(err) = run_cli() {
        eprintln!("error: {err}");
        for cause in err.chain().skip(1) {
            eprintln!("  caused by: {cause}");
        }
        process::exit(1);
    }
}

fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Counter) => run_single("counter"),
        Some(Commands::HelloTimer) => run_single("hello-timer"),
        Some(Commands::BlobEcho) => run_single("blob-echo"),
        Some(Commands::All) => run_all(),
        None => {
            list_examples();
            Ok(())
        }
    }
}

fn list_examples() {
    println!("Available examples:\n");
    for ex in EXAMPLES {
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
    match slug {
        "counter" => examples::counter::run(&abs_dir),
        "hello-timer" => Err(anyhow!("hello timer example not implemented yet")),
        "blob-echo" => Err(anyhow!("blob echo example not implemented yet")),
        other => Err(anyhow!("example '{other}' not wired up")),
    }
}

fn run_all() -> Result<()> {
    for ex in EXAMPLES {
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
