//! `aos world gov` governance commands (stubs).

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use crate::commands::gov_control::send_req;
use crate::opts::{WorldOpts, resolve_dirs};
use crate::util::validate_patch_json;
use aos_air_types::AirNode;
use aos_cbor::Hash;
use aos_host::{control::ControlClient, manifest_loader::ZERO_HASH_SENTINEL};
use base64::prelude::*;
use std::collections::HashMap;

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

    /// Require all hashes to be provided (disable auto-fill of zero/missing hashes)
    #[arg(long, default_value_t = false, help = "Enforce that all manifest refs in patch doc carry non-zero hashes; disable client auto-fill")]
    pub require_hashes: bool,
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

            let patch_bytes = if json.get("patches").is_some() {
                let mut doc = json;
                autofill_patchdoc_hashes(&mut doc, propose_args.require_hashes)?;
                validate_patch_json(&doc)?;
                println!(
                    "PatchDoc validated{}",
                    if propose_args.require_hashes {
                        " (hashes enforced)"
                    } else {
                        " (hashes auto-filled where zero/missing)"
                    }
                );
                // Send JSON bytes; server will accept PatchDocument JSON.
                serde_json::to_vec(&doc).context("encode patch JSON")?
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

fn autofill_patchdoc_hashes(doc: &mut serde_json::Value, require_hashes: bool) -> Result<()> {
    let Some(patches) = doc.get_mut("patches").and_then(|v| v.as_array_mut()) else {
        return Ok(());
    };

    // Collect hashes for new/updated defs in this document.
    let mut known_hashes: HashMap<String, String> = HashMap::new();
    for patch in patches.iter() {
        if let Some(add_def) = patch.get("add_def") {
            if let Some(node) = add_def.get("node") {
                if let Ok(node_air) = serde_json::from_value::<AirNode>(node.clone()) {
                    if let Some(name) = node_name(&node_air) {
                        known_hashes.insert(name.to_string(), Hash::of_cbor(&node_air)?.to_hex());
                    }
                }
            }
        }
        if let Some(repl) = patch.get("replace_def") {
            if let Some(node) = repl.get("new_node") {
                if let Ok(node_air) = serde_json::from_value::<AirNode>(node.clone()) {
                    if let Some(name) = node_name(&node_air) {
                        known_hashes.insert(name.to_string(), Hash::of_cbor(&node_air)?.to_hex());
                    }
                }
            }
        }
    }

    for patch in patches.iter_mut() {
        if let Some(set_refs) = patch.get_mut("set_manifest_refs") {
            if let Some(add) = set_refs.get_mut("add").and_then(|v| v.as_array_mut()) {
                for entry in add.iter_mut() {
                    let obj = entry
                        .as_object_mut()
                        .ok_or_else(|| anyhow::anyhow!("manifest ref entry must be object"))?;
                    let name = obj
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow::anyhow!("manifest ref missing name"))?;
                    let needs_fill = match obj.get("hash").and_then(|v| v.as_str()) {
                        None => true,
                        Some(h) if h == ZERO_HASH_SENTINEL => true,
                        Some(_) => false,
                    };
                    if needs_fill {
                        if let Some(h) = known_hashes.get(name) {
                            obj.insert("hash".into(), serde_json::Value::String(h.clone()));
                        } else if require_hashes {
                            anyhow::bail!(
                                "hash missing for manifest ref '{}' and --require-hashes is set",
                                name
                            );
                        }
                    }
                }
            }
        }
    }

    if require_hashes {
        for patch in patches.iter() {
            if let Some(set_refs) = patch.get("set_manifest_refs") {
                if let Some(add) = set_refs.get("add").and_then(|v| v.as_array()) {
                    for entry in add {
                        if let Some(h) = entry.get("hash").and_then(|v| v.as_str()) {
                            if h == ZERO_HASH_SENTINEL {
                                anyhow::bail!(
                                    "hash still zero for manifest ref '{}'",
                                    entry
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("<unknown>")
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn node_name(node: &AirNode) -> Option<&str> {
    match node {
        AirNode::Defmodule(n) => Some(n.name.as_str()),
        AirNode::Defplan(n) => Some(n.name.as_str()),
        AirNode::Defschema(n) => Some(n.name.as_str()),
        AirNode::Defcap(n) => Some(n.name.as_str()),
        AirNode::Defpolicy(n) => Some(n.name.as_str()),
        AirNode::Defeffect(n) => Some(n.name.as_str()),
        AirNode::Defsecret(n) => Some(n.name.as_str()),
        AirNode::Manifest(_) => None,
    }
}
