use anyhow::Result;
use clap::{Args, Subcommand};

use crate::GlobalOpts;
use crate::client::ApiClient;
use crate::output::{OutputOpts, print_success};

use super::common::resolve_target;

#[derive(Args, Debug)]
#[command(about = "Inspect service health and worker placement state")]
pub(crate) struct OpsArgs {
    #[command(subcommand)]
    cmd: OpsCommand,
}

#[derive(Subcommand, Debug)]
enum OpsCommand {
    /// Show basic service health information.
    Health,
}

pub(crate) async fn handle(global: &GlobalOpts, output: OutputOpts, args: OpsArgs) -> Result<()> {
    let target = resolve_target(global)?;
    let client = ApiClient::new(&target)?;
    match args.cmd {
        OpsCommand::Health => {
            let data = client.get_json("/v1/health", &[]).await?;
            print_success(output, data, None, vec![])
        }
    }
}
