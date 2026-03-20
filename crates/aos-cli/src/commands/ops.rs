use anyhow::Result;
use clap::{Args, Subcommand};

use crate::GlobalOpts;
use crate::client::ApiClient;
use crate::output::{OutputOpts, print_success};

use super::common::{resolve_selected_universe, resolve_target};

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
    /// List active workers in the selected universe.
    Workers,
    /// List worlds currently assigned to one worker.
    WorkerWorlds(WorkerWorldsArgs),
}

#[derive(Args, Debug)]
struct WorkerWorldsArgs {
    /// Worker identifier to inspect.
    worker_id: String,
}

pub(crate) async fn handle(global: &GlobalOpts, output: OutputOpts, args: OpsArgs) -> Result<()> {
    let target = resolve_target(global)?;
    let client = ApiClient::new(&target)?;
    match args.cmd {
        OpsCommand::Health => {
            let data = client.get_json("/v1/health", &[]).await?;
            print_success(output, data, None, vec![])
        }
        OpsCommand::Workers => {
            let universe = resolve_selected_universe(&client, &target).await?;
            let data = client
                .get_json(&format!("/v1/universes/{universe}/workers"), &[])
                .await?;
            print_success(output, data, None, vec![])
        }
        OpsCommand::WorkerWorlds(args) => {
            let universe = resolve_selected_universe(&client, &target).await?;
            let data = client
                .get_json(
                    &format!("/v1/universes/{universe}/workers/{}/worlds", args.worker_id),
                    &[],
                )
                .await?;
            print_success(output, data, None, vec![])
        }
    }
}
