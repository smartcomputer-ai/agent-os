pub(crate) mod cas;
pub(crate) mod common;
pub(crate) mod node;
pub(crate) mod ops;
pub(crate) mod profile;
pub(crate) mod universe;
pub(crate) mod workspace;
pub(crate) mod world;

use anyhow::Result;
use clap::Subcommand;

use crate::GlobalOpts;
use crate::output::OutputOpts;

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
    /// Manage saved CLI profiles and the current target selection.
    #[command(visible_aliases = ["p", "profiles"])]
    Profile(profile::ProfileArgs),
    /// Manage the AgentOS node runtime and target selection.
    Node(node::NodeArgs),
    /// Internal node server entrypoint used by `aos node up`.
    #[command(name = "node-serve", hide = true)]
    NodeServe(node::NodeServeArgs),
    /// Manage node secret bindings and secret versions.
    #[command(visible_aliases = ["u", "universes"])]
    Universe(universe::UniverseArgs),
    /// Manage worlds, governance, events, and world queries.
    #[command(visible_aliases = ["w", "worlds"])]
    World(world::WorldArgs),
    /// Inspect and synchronize node workspaces.
    #[command(visible_alias = "ws")]
    Workspace(workspace::WorkspaceArgs),
    /// Interact with the universe CAS directly.
    Cas(cas::CasArgs),
    /// Inspect service state.
    Ops(ops::OpsArgs),
}

pub(crate) async fn dispatch(
    global: &GlobalOpts,
    output: OutputOpts,
    command: Command,
) -> Result<()> {
    match command {
        Command::Profile(args) => profile::handle(global, output, args).await,
        Command::Node(args) => node::handle(global, output, args).await,
        Command::NodeServe(args) => node::handle_node_serve(args).await,
        Command::Universe(args) => universe::handle(global, output, args).await,
        Command::World(args) => world::handle(global, output, args).await,
        Command::Workspace(args) => workspace::handle(global, output, args).await,
        Command::Cas(args) => cas::handle(global, output, args).await,
        Command::Ops(args) => ops::handle(global, output, args).await,
    }
}
