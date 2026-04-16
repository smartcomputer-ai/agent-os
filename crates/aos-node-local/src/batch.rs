use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use aos_cbor::to_canonical_cbor;
use aos_node::{DomainEventIngress, WorldId};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use clap::{Args, Subcommand};
use serde::Serialize;

use crate::{LocalControl, LocalStatePaths};

#[derive(Args, Debug, Clone)]
pub struct BatchArgs {
    #[arg(long, env = "AOS_LOCAL_STATE_ROOT", default_value = ".aos")]
    pub state_root: PathBuf,

    #[command(subcommand)]
    pub command: BatchCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum BatchCommand {
    /// List persisted local worlds.
    Worlds,
    /// Show the current runtime/admin summary for one local world.
    Status(WorldTargetArgs),
    /// Create a fresh local checkpoint for one world and print the updated summary.
    Checkpoint(WorldTargetArgs),
    /// Pump one local world until quiescent and print the updated summary.
    Step(WorldTargetArgs),
    /// Load and print the live manifest for one local world.
    Manifest(WorldTargetArgs),
    /// Print the workflow trace summary for one local world.
    TraceSummary(WorldTargetArgs),
    /// Enqueue one domain event and immediately run the world to quiescence.
    Send(BatchSendArgs),
    /// Submit or inspect one local command directly against persisted local state.
    Command(BatchCommandArgs),
}

#[derive(Args, Debug, Clone)]
pub struct WorldTargetArgs {
    /// World UUID.
    #[arg(long)]
    pub world: String,
}

#[derive(Args, Debug, Clone)]
pub struct BatchSendArgs {
    #[command(flatten)]
    pub target: WorldTargetArgs,

    /// Event schema name.
    #[arg(long)]
    pub schema: String,

    /// JSON event value provided inline.
    #[arg(long)]
    pub value_json: Option<String>,

    /// File containing the JSON event value.
    #[arg(long)]
    pub value_file: Option<PathBuf>,

    /// Raw CBOR event value encoded as base64.
    #[arg(long)]
    pub value_b64: Option<String>,

    /// Optional event key encoded as base64 CBOR bytes.
    #[arg(long)]
    pub key_b64: Option<String>,

    /// Optional correlation identifier.
    #[arg(long)]
    pub correlation_id: Option<String>,
}

#[derive(Args, Debug, Clone)]
pub struct BatchCommandArgs {
    #[command(subcommand)]
    pub command: BatchCommandSubcommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum BatchCommandSubcommand {
    /// Submit one command and print the resulting command record.
    Submit(BatchCommandSubmitArgs),
    /// Fetch one existing command record.
    Get(BatchCommandGetArgs),
}

#[derive(Args, Debug, Clone)]
pub struct BatchCommandSubmitArgs {
    #[command(flatten)]
    pub target: WorldTargetArgs,

    /// Command name.
    #[arg(long)]
    pub command: String,

    /// Optional stable command identifier.
    #[arg(long)]
    pub command_id: Option<String>,

    /// Optional actor recorded on the command.
    #[arg(long)]
    pub actor: Option<String>,

    /// JSON command payload provided inline.
    #[arg(long)]
    pub payload_json: Option<String>,

    /// File containing the JSON command payload.
    #[arg(long)]
    pub payload_file: Option<PathBuf>,

    /// Raw CBOR command payload encoded as base64.
    #[arg(long)]
    pub payload_b64: Option<String>,
}

#[derive(Args, Debug, Clone)]
pub struct BatchCommandGetArgs {
    #[command(flatten)]
    pub target: WorldTargetArgs,

    /// Command identifier.
    #[arg(long)]
    pub command_id: String,
}

#[derive(Debug, Serialize)]
struct BatchEventResponse {
    seq: String,
    world: aos_node::api::WorldSummaryResponse,
}

pub fn run_batch(args: BatchArgs) -> Result<()> {
    let paths = LocalStatePaths::new(args.state_root);
    paths
        .ensure_root()
        .with_context(|| format!("create local state root {}", paths.root().display()))?;
    let control = LocalControl::open_batch(paths.root()).context("open local batch control")?;
    match args.command {
        BatchCommand::Worlds => print_json(&control.list_worlds(None, u32::MAX)?),
        BatchCommand::Status(target) => {
            let world = resolve_world_id(&control, &target.world)?;
            print_json(&control.get_world(world)?)
        }
        BatchCommand::Checkpoint(target) => {
            let world = resolve_world_id(&control, &target.world)?;
            print_json(&control.checkpoint_world(world)?)
        }
        BatchCommand::Step(target) => {
            let world = resolve_world_id(&control, &target.world)?;
            print_json(&control.step_world(world)?)
        }
        BatchCommand::Manifest(target) => {
            let world = resolve_world_id(&control, &target.world)?;
            print_json(&control.manifest(world)?)
        }
        BatchCommand::TraceSummary(target) => {
            let world = resolve_world_id(&control, &target.world)?;
            print_json(&control.trace_summary(world, 64)?)
        }
        BatchCommand::Send(args) => run_send(&control, args),
        BatchCommand::Command(args) => run_command(&control, args),
    }
}

fn run_send(control: &Arc<LocalControl>, args: BatchSendArgs) -> Result<()> {
    let world = resolve_world_id(control, &args.target.world)?;
    let value = load_cbor_payload(
        args.value_json.as_deref(),
        args.value_file.as_deref(),
        args.value_b64.as_deref(),
        "event value",
    )?;
    let key = args
        .key_b64
        .as_deref()
        .map(decode_b64)
        .transpose()
        .context("decode event key")?;
    let seq = control.enqueue_event(
        world,
        DomainEventIngress {
            schema: args.schema,
            value: aos_node::CborPayload::inline(value),
            key,
            correlation_id: args.correlation_id,
        },
    )?;
    let world = control.get_world(world)?;
    print_json(&BatchEventResponse {
        seq: seq.to_string(),
        world,
    })
}

fn run_command(control: &Arc<LocalControl>, args: BatchCommandArgs) -> Result<()> {
    match args.command {
        BatchCommandSubcommand::Submit(args) => {
            let world = resolve_world_id(control, &args.target.world)?;
            let payload = load_cbor_payload(
                args.payload_json.as_deref(),
                args.payload_file.as_deref(),
                args.payload_b64.as_deref(),
                "command payload",
            )?;
            let payload: serde_cbor::Value =
                serde_cbor::from_slice(&payload).context("decode command CBOR payload")?;
            let response = control.submit_command(
                world,
                &args.command,
                args.command_id.clone(),
                args.actor.clone(),
                &payload,
            )?;
            let record = control.get_command(world, &response.command_id)?;
            print_json(&record)
        }
        BatchCommandSubcommand::Get(args) => {
            let world = resolve_world_id(control, &args.target.world)?;
            print_json(&control.get_command(world, &args.command_id)?)
        }
    }
}

fn resolve_world_id(control: &Arc<LocalControl>, selector: &str) -> Result<WorldId> {
    let world_id = selector
        .parse::<WorldId>()
        .with_context(|| format!("parse local world id '{selector}'"))?;
    let _ = control
        .get_world(world_id)
        .with_context(|| format!("resolve local world '{selector}'"))?;
    Ok(world_id)
}

fn load_cbor_payload(
    inline_json: Option<&str>,
    json_file: Option<&std::path::Path>,
    raw_b64: Option<&str>,
    label: &str,
) -> Result<Vec<u8>> {
    let provided = [
        inline_json.is_some(),
        json_file.is_some(),
        raw_b64.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if provided != 1 {
        bail!("{label} requires exactly one input source");
    }
    if let Some(text) = inline_json {
        let value: serde_json::Value =
            serde_json::from_str(text).with_context(|| format!("parse {label} json"))?;
        return Ok(to_canonical_cbor(&value)?);
    }
    if let Some(path) = json_file {
        let text = fs::read_to_string(path)
            .with_context(|| format!("read {label} json file {}", path.display()))?;
        let value: serde_json::Value =
            serde_json::from_str(&text).with_context(|| format!("parse {label} json file"))?;
        return Ok(to_canonical_cbor(&value)?);
    }
    decode_b64(raw_b64.expect("one payload input is required"))
        .with_context(|| format!("decode {label} base64 cbor"))
}

fn decode_b64(value: &str) -> Result<Vec<u8>> {
    BASE64_STANDARD
        .decode(value)
        .map_err(|err| anyhow!("invalid base64: {err}"))
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::load_cbor_payload;

    #[test]
    fn load_cbor_payload_requires_exactly_one_input() {
        let err = load_cbor_payload(None, None, None, "event value").unwrap_err();
        assert!(err.to_string().contains("exactly one"));
    }
}
