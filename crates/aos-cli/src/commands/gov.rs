//! `aos world gov` governance commands (stubs).

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde_cbor;

use crate::commands::gov_control::send_req;
use crate::opts::{WorldOpts, resolve_dirs};
use crate::util::validate_patch_json;
use aos_host::control::ControlClient;
use base64::prelude::*;

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

    /// Optional description
    #[arg(long)]
    pub description: Option<String>,
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

    /// Approver identity
    #[arg(long, default_value = "cli")]
    pub approver: String,
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
    let dirs = resolve_dirs(opts)?;

    match &args.cmd {
        GovSubcommand::Propose(propose_args) => {
            // Validate patch file exists
            if !propose_args.patch.exists() {
                anyhow::bail!("patch file not found: {}", propose_args.patch.display());
            }
            let text = std::fs::read_to_string(&propose_args.patch).context("read patch file")?;
            let json: serde_json::Value =
                serde_json::from_str(&text).context("parse patch JSON")?;

            if json.get("patches").is_some() {
                validate_patch_json(&json)?;
                println!("Patch validated against patch.schema.json");
            } else {
                println!(
                    "Patch has no 'patches' field; skipping patch.schema.json validation (authoring sugar manifest patch?)."
                );
            }

            // For now expect CBOR patch bytes in the file if not a JSON patch envelope.
            let patch_bytes = if json.get("patches").is_some() {
                // Submit JSON patch by serializing to CBOR (kernel will canonicalize internally).
                serde_cbor::to_vec(&json).context("encode patch JSON to CBOR")?
            } else {
                // treat as raw CBOR
                fs::read(&propose_args.patch).context("read patch cbor")?
            };

            let mut client = ControlClient::connect(&dirs.control_socket())
                .await
                .context("connect control socket")?;
            let resp = send_req(
                &mut client,
                "propose",
                serde_json::json!({
                    "patch_b64": BASE64_STANDARD.encode(patch_bytes),
                    "description": propose_args.description
                }),
            )
            .await?;
            println!("Proposed: {}", resp.result.unwrap_or_default());
        }
        GovSubcommand::Shadow(shadow_args) => {
            let proposal_id: u64 = shadow_args.id.parse().context("proposal id must be u64")?;
            let mut client = ControlClient::connect(&dirs.control_socket())
                .await
                .context("connect control socket")?;
            let resp = send_req(
                &mut client,
                "shadow",
                serde_json::json!({ "proposal_id": proposal_id }),
            )
            .await?;
            println!(
                "Shadow summary: {}",
                resp.result
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "{}".into())
            );
        }
        GovSubcommand::Approve(approve_args) => {
            let proposal_id: u64 = approve_args.id.parse().context("proposal id must be u64")?;
            let mut client = ControlClient::connect(&dirs.control_socket())
                .await
                .context("connect control socket")?;
            let resp = send_req(
                &mut client,
                "approve",
                serde_json::json!({
                    "proposal_id": proposal_id,
                    "decision": approve_args.decision,
                    "approver": approve_args.approver
                }),
            )
            .await?;
            println!("Approve result: ok={}", resp.ok);
        }
        GovSubcommand::Apply(apply_args) => {
            let proposal_id: u64 = apply_args.id.parse().context("proposal id must be u64")?;
            let mut client = ControlClient::connect(&dirs.control_socket())
                .await
                .context("connect control socket")?;
            let resp = send_req(
                &mut client,
                "apply",
                serde_json::json!({ "proposal_id": proposal_id }),
            )
            .await?;
            println!("Apply result: ok={}", resp.ok);
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
