//! `aos gov` governance commands.

use std::fs;
use std::path::PathBuf;

use crate::opts::{WorldOpts, resolve_dirs};
use crate::output::print_success;
use crate::util::validate_patch_json;
use anyhow::{Context, Result};
use aos_air_types::AirNode;
use aos_cbor::Hash;
use aos_host::control::{ControlClient, RequestEnvelope, ResponseEnvelope};
use aos_host::manifest_loader::ZERO_HASH_SENTINEL;
use aos_host::world_io::{BundleFilter, build_patch_document, load_air_bundle, resolve_base_manifest};
use aos_store::FsStore;
use base64::prelude::*;
use clap::{Args, Subcommand};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

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

    /// Get proposal details
    Get(GetArgs),
}

#[derive(Args, Debug)]
pub struct ProposeArgs {
    /// Path to patch file (PatchDocument JSON or ManifestPatch CBOR)
    #[arg(long, conflicts_with = "patch_dir")]
    pub patch: Option<PathBuf>,

    /// Build a PatchDocument from an AIR directory (compute hashes, set manifest refs)
    #[arg(long, conflicts_with = "patch", hide = true)]
    pub patch_dir: Option<PathBuf>,

    /// Optional base manifest hash; defaults to current world manifest if omitted
    #[arg(long)]
    pub base: Option<String>,

    /// Optional description
    #[arg(long)]
    pub description: Option<String>,

    /// Require all hashes to be provided (disable auto-fill of zero/missing hashes)
    #[arg(
        long,
        default_value_t = false,
        help = "Enforce that all manifest refs in patch doc carry non-zero hashes; disable client auto-fill"
    )]
    pub require_hashes: bool,

    /// Dry-run: print the generated PatchDocument JSON and exit
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
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
pub struct GetArgs {
    /// Proposal ID
    #[arg(long)]
    pub id: String,
}

pub async fn cmd_gov(opts: &WorldOpts, args: &GovArgs) -> Result<()> {
    // Validate world exists (governance operates on a specific world)
    let dirs = resolve_dirs(opts)?;

    match &args.cmd {
        GovSubcommand::Propose(propose_args) => {
            let mut control_client = if propose_args.dry_run {
                if super::should_use_control(opts) {
                    super::try_control_client(&dirs).await
                } else {
                    None
                }
            } else {
                Some(
                    ControlClient::connect(&dirs.control_socket)
                        .await
                        .context("connect control socket")?,
                )
            };

            let patch_bytes = if let Some(dir) = &propose_args.patch_dir {
                eprintln!(
                    "notice: --patch-dir is deprecated; use `aos import --air <dir> --mode patch --air-only --propose`"
                );
                let store = Arc::new(FsStore::open(&dirs.store_root)?);
                let bundle = load_air_bundle(store.clone(), dir, BundleFilter::AirOnly)?;
                let manifest_path = dirs.store_root.join(".aos/manifest.air.cbor");
                let base_manifest = resolve_base_manifest(
                    store.as_ref(),
                    propose_args.base.clone(),
                    control_client.as_mut(),
                    &manifest_path,
                )
                .await?;
                let doc = build_patch_document(
                    &bundle,
                    &base_manifest.manifest,
                    &base_manifest.hash,
                )?;
                let mut doc_json = serde_json::to_value(&doc).context("serialize patch doc")?;
                autofill_patchdoc_hashes(&mut doc_json, propose_args.require_hashes)?;
                validate_patch_json(&doc_json)?;
                if propose_args.dry_run {
                    println!("{}", serde_json::to_string_pretty(&doc_json)?);
                    return Ok(());
                }
                serde_json::to_vec(&doc_json).context("encode patch JSON")?
            } else {
                let patch_path = propose_args
                    .patch
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("--patch or --patch-dir is required"))?;
                if !patch_path.exists() {
                    anyhow::bail!("patch file not found: {}", patch_path.display());
                }
                let text = std::fs::read_to_string(patch_path).context("read patch file")?;
                let json: serde_json::Value =
                    serde_json::from_str(&text).context("parse patch JSON")?;

                if json.get("patches").is_some() {
                    let mut doc = json;
                    autofill_patchdoc_hashes(&mut doc, propose_args.require_hashes)?;
                    validate_patch_json(&doc)?;
                    if propose_args.dry_run {
                        println!("{}", serde_json::to_string_pretty(&doc)?);
                        return Ok(());
                    }
                    serde_json::to_vec(&doc).context("encode patch JSON")?
                } else {
                    if propose_args.dry_run {
                        println!("(dry-run) raw CBOR patch file {}", patch_path.display());
                        return Ok(());
                    }
                    // treat as raw CBOR
                    fs::read(patch_path).context("read patch cbor")?
                }
            };

            if propose_args.dry_run {
                return Ok(());
            }
            let mut client = if let Some(client) = control_client.take() {
                client
            } else {
                ControlClient::connect(&dirs.control_socket)
                    .await
                    .context("connect control socket")?
            };
            let resp = send_req(
                &mut client,
                "gov-propose",
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
            let mut client = ControlClient::connect(&dirs.control_socket)
                .await
                .context("connect control socket")?;
            let resp = send_req(
                &mut client,
                "gov-shadow",
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
            let mut client = ControlClient::connect(&dirs.control_socket)
                .await
                .context("connect control socket")?;
            let resp = send_req(
                &mut client,
                "gov-approve",
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
            let mut client = ControlClient::connect(&dirs.control_socket)
                .await
                .context("connect control socket")?;
            let resp = send_req(
                &mut client,
                "gov-apply",
                serde_json::json!({ "proposal_id": proposal_id }),
            )
            .await?;
            println!("Apply result: ok={}", resp.ok);
        }
        GovSubcommand::List(list_args) => {
            let mut client = ControlClient::connect(&dirs.control_socket)
                .await
                .context("connect control socket")?;
            let resp = send_req(
                &mut client,
                "gov-list",
                serde_json::json!({ "status": list_args.status }),
            )
            .await?;
            let result = resp.result.unwrap_or_default();
            let proposals = result
                .get("proposals")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([]));
            let meta = result.get("meta").cloned();
            print_success(opts, proposals, meta, vec![])?;
        }
        GovSubcommand::Get(get_args) => {
            let proposal_id: u64 = get_args.id.parse().context("proposal id must be u64")?;
            let mut client = ControlClient::connect(&dirs.control_socket)
                .await
                .context("connect control socket")?;
            let resp = send_req(
                &mut client,
                "gov-get",
                serde_json::json!({ "proposal_id": proposal_id }),
            )
            .await?;
            let result = resp.result.unwrap_or_default();
            let proposal = result
                .get("proposal")
                .cloned()
                .unwrap_or_else(|| serde_json::json!(null));
            let meta = result.get("meta").cloned();
            print_success(opts, proposal, meta, vec![])?;
        }
    }

    Ok(())
}

// Exposed for tests.
pub fn autofill_patchdoc_hashes(doc: &mut serde_json::Value, require_hashes: bool) -> Result<()> {
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


pub async fn send_req(
    client: &mut ControlClient,
    cmd: &str,
    payload: Value,
) -> Result<ResponseEnvelope> {
    let env = RequestEnvelope {
        v: 1,
        id: format!("gov-{cmd}"),
        cmd: cmd.into(),
        payload,
    };
    let resp = client.request(&env).await?;
    if !resp.ok {
        let msg = resp
            .error
            .as_ref()
            .map(|e| format!("{}: {}", e.code, e.message))
            .unwrap_or_else(|| "unknown error".into());
        anyhow::bail!("control {} failed: {}", cmd, msg);
    }
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autofill_fills_zero_hashes_by_default() {
        let mut doc = serde_json::json!({
            "base_manifest_hash": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "patches": [
                { "add_def": { "kind": "defschema", "node": { "$kind":"defschema", "name":"demo/Added@1", "type": { "bool": {} } } } },
                { "set_manifest_refs": { "add": [ { "kind":"defschema", "name":"demo/Added@1", "hash": ZERO_HASH_SENTINEL } ] } }
            ]
        });

        autofill_patchdoc_hashes(&mut doc, false).expect("autofill");
        let filled = doc["patches"][1]["set_manifest_refs"]["add"][0]["hash"]
            .as_str()
            .expect("hash present");
        assert_ne!(filled, ZERO_HASH_SENTINEL, "hash should be filled");
        assert!(
            filled.starts_with("sha256:"),
            "hash should be canonical sha256 prefix"
        );
    }

    #[test]
    fn require_hashes_rejects_zero_hashes() {
        let mut doc = serde_json::json!({
            "base_manifest_hash": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "patches": [
                { "set_manifest_refs": { "add": [ { "kind":"defschema", "name":"demo/Added@1", "hash": ZERO_HASH_SENTINEL } ] } }
            ]
        });

        let err = autofill_patchdoc_hashes(&mut doc, true)
            .expect_err("should fail when hashes remain zero");
        assert!(
            err.to_string().contains("hash missing") || err.to_string().contains("hash still zero"),
            "require-hashes should error on zero hashes, got {err}"
        );
    }
}
