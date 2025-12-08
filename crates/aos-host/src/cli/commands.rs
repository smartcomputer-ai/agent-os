use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Init {
        path: PathBuf,
    },
    Step {
        path: PathBuf,
        #[arg(long)]
        event: Option<String>,
        #[arg(long)]
        value: Option<String>,
    },
}
