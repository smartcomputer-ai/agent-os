//! `aos world gov` governance commands (stubs).

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

use crate::opts::{resolve_dirs, WorldOpts};
use crate::util::validate_patch_json;

#[derive(Args, Debug)]
pub struct GovArgs {
    #[command(subcommand)]
    pub cmd: GovSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum GovSubcommand {
    /// Submit a governance proposal
    Propose(ProposeArgs),

    /// Run shadow evaluation of a proposal
    Shadow(ShadowArgs),

    /// Approve or reject a proposal
    Approve(ApproveArgs),

    /// Apply an approved proposal
    Apply(ApplyArgs),

    /// List governance proposals
    List(ListArgs),

    /// Show proposal details
    Show(ShowArgs),
}

#[derive(Args, Debug)]
pub struct ProposeArgs {
    /// Path to patch file
    #[arg(long)]
    pub patch: PathBuf,
}

#[derive(Args, Debug)]
pub struct ShadowArgs {
    /// Proposal ID
    #[arg(long)]
    pub id: String,
}

#[derive(Args, Debug)]
pub struct ApproveArgs {
    /// Proposal ID
    #[arg(long)]
    pub id: String,

    /// Decision (approve or reject)
    #[arg(long, default_value = "approve")]
    pub decision: String,
}

#[derive(Args, Debug)]
pub struct ApplyArgs {
    /// Proposal ID
    #[arg(long)]
    pub id: String,
}

#[derive(Args, Debug)]
pub struct ListArgs {
    /// Filter by status (pending, approved, applied, rejected, all)
    #[arg(long, default_value = "pending")]
    pub status: String,
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// Proposal ID
    #[arg(long)]
    pub id: String,
}

pub async fn cmd_gov(opts: &WorldOpts, args: &GovArgs) -> Result<()> {
    // Validate world exists (governance operates on a specific world)
    let _dirs = resolve_dirs(opts)?;

    match &args.cmd {
        GovSubcommand::Propose(propose_args) => {
            // Validate patch file exists
            if !propose_args.patch.exists() {
                anyhow::bail!("patch file not found: {}", propose_args.patch.display());
            }
            let text = std::fs::read_to_string(&propose_args.patch)
                .context("read patch file")?;
            let json: serde_json::Value =
                serde_json::from_str(&text).context("parse patch JSON")?;

            if json.get("patches").is_some() {
                validate_patch_json(&json)?;
                println!("Patch validated against patch.schema.json");
            } else {
                println!("Patch has no 'patches' field; skipping patch.schema.json validation (authoring sugar manifest patch?).");
            }

            println!("Governance not yet implemented; patch ready for submission");
        }
        GovSubcommand::Shadow(shadow_args) => {
            println!(
                "Governance not yet implemented.\n\
                 Would run shadow for proposal: {}",
                shadow_args.id
            );
        }
        GovSubcommand::Approve(approve_args) => {
            println!(
                "Governance not yet implemented.\n\
                 Would {} proposal: {}",
                approve_args.decision, approve_args.id
            );
        }
        GovSubcommand::Apply(apply_args) => {
            println!(
                "Governance not yet implemented.\n\
                 Would apply proposal: {}",
                apply_args.id
            );
        }
        GovSubcommand::List(list_args) => {
            println!(
                "Governance not yet implemented.\n\
                 Would list proposals with status: {}",
                list_args.status
            );
        }
        GovSubcommand::Show(show_args) => {
            println!(
                "Governance not yet implemented.\n\
                 Would show proposal: {}",
                show_args.id
            );
        }
    }

    Ok(())
}
