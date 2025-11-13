use clap::{Parser, Subcommand};
use std::path::Path;
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
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Commands::Counter) => run_single("counter"),
        Some(Commands::HelloTimer) => run_single("hello-timer"),
        Some(Commands::BlobEcho) => run_single("blob-echo"),
        Some(Commands::All) => run_all(),
        None => {
            list_examples();
            Ok(())
        }
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        process::exit(1);
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

fn run_single(slug: &str) -> Result<(), String> {
    let ex = EXAMPLES
        .iter()
        .find(|ex| ex.slug == slug)
        .ok_or_else(|| format!("unknown example '{slug}'"))?;
    println!(
        "Running example {number} — {title} ({slug})",
        number = ex.number,
        title = ex.title,
        slug = ex.slug
    );
    ensure_structure_exists(ex)?;
    println!("  runner: {}/runner", ex.dir);
    println!("  status: not yet implemented\n");
    Ok(())
}

fn run_all() -> Result<(), String> {
    for ex in EXAMPLES {
        run_single(ex.slug)?;
    }
    Ok(())
}

fn ensure_structure_exists(ex: &ExampleMeta) -> Result<(), String> {
    if !Path::new(ex.dir).exists() {
        return Err(format!("missing directory '{}'", ex.dir));
    }
    Ok(())
}
