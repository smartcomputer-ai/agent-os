use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "aos", version, about = "AgentOS CLI (skeleton)")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Init {},
    Run {},
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Init {} => println!("init: not yet implemented"),
        Commands::Run {} => println!("run: not yet implemented"),
    }
}
