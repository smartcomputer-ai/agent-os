//! `aos gov` governance commands (stubs).

use std::fs;
use std::path::PathBuf;

use crate::opts::{ResolvedDirs, WorldOpts, resolve_dirs};
use crate::util::validate_patch_json;
use anyhow::{Context, Result};
use aos_air_types::AirNode;
use aos_cbor::Hash;
use aos_host::control::{ControlClient, RequestEnvelope, ResponseEnvelope};
use aos_host::manifest_loader::ZERO_HASH_SENTINEL;
use aos_host::manifest_loader::load_from_assets;
use aos_store::{FsStore, Store};
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

    /// Show proposal details
    Show(ShowArgs),
}

#[derive(Args, Debug)]
pub struct ProposeArgs {
    /// Path to patch file (PatchDocument JSON or ManifestPatch CBOR)
    #[arg(long, conflicts_with = "patch_dir")]
    pub patch: Option<PathBuf>,

    /// Build a PatchDocument from an AIR directory (compute hashes, set manifest refs)
    #[arg(long, conflicts_with = "patch")]
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
            let patch_bytes = if let Some(dir) = &propose_args.patch_dir {
                let doc = build_patchdoc_from_dir(&dirs, dir, propose_args.base.clone())?;
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

            let mut client = ControlClient::connect(&dirs.control_socket)
                .await
                .context("connect control socket")?;
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

fn build_patchdoc_from_dir(
    dirs: &ResolvedDirs,
    air_dir: &PathBuf,
    base_override: Option<String>,
) -> Result<serde_json::Value> {
    let store = Arc::new(FsStore::open(&dirs.store_root)?);
    let loaded = load_from_assets(store.clone(), air_dir)
        .context("load AIR from patch-dir")?
        .ok_or_else(|| anyhow::anyhow!("no manifest found under {}", air_dir.display()))?;

    // Derive base manifest hash: CLI defaults to current world manifest.air.cbor unless overridden.
    let base_manifest_hash = if let Some(h) = base_override {
        h
    } else {
        let manifest_path = dirs.store_root.join(".aos/manifest.air.cbor");
        let bytes = fs::read(&manifest_path).with_context(|| {
            format!(
                "read current world manifest at {} (or pass --base)",
                manifest_path.display()
            )
        })?;
        Hash::of_bytes(&bytes).to_hex()
    };
    let base_manifest = {
        let h = Hash::from_hex_str(&base_manifest_hash).context("parse base manifest hash")?;
        match store.get_node(h) {
            Ok(AirNode::Manifest(m)) => m,
            Ok(_) => anyhow::bail!("base_manifest_hash does not point to a manifest"),
            Err(_) => {
                // Fallback: parse manifest bytes directly if node not in store (e.g., test fixture)
                let manifest_path = dirs.store_root.join(".aos/manifest.air.cbor");
                let bytes = fs::read(&manifest_path)?;
                if let Ok(catalog) = aos_store::load_manifest_from_bytes(store.as_ref(), &bytes) {
                    catalog.manifest
                } else {
                    serde_json::from_slice(&bytes).context("parse manifest json")?
                }
            }
        }
    };

    // Build add_def ops for all defs in the loaded bundle.
    let mut patches: Vec<serde_json::Value> = Vec::new();
    for node in loaded
        .modules
        .values()
        .cloned()
        .map(AirNode::Defmodule)
        .chain(loaded.plans.values().cloned().map(AirNode::Defplan))
        .chain(loaded.schemas.values().cloned().map(AirNode::Defschema))
        .chain(loaded.caps.values().cloned().map(AirNode::Defcap))
        .chain(loaded.policies.values().cloned().map(AirNode::Defpolicy))
        .chain(loaded.effects.values().cloned().map(AirNode::Defeffect))
    {
        let kind = match &node {
            AirNode::Defmodule(_) => "defmodule",
            AirNode::Defplan(_) => "defplan",
            AirNode::Defschema(_) => "defschema",
            AirNode::Defcap(_) => "defcap",
            AirNode::Defpolicy(_) => "defpolicy",
            AirNode::Defeffect(_) => "defeffect",
            AirNode::Defsecret(_) | AirNode::Manifest(_) => continue, // skip secrets/manifest for now
        };
        patches.push(serde_json::json!({
            "add_def": { "kind": kind, "node": serde_json::to_value(&node)? }
        }));
    }

    // Set manifest refs from the loaded manifest.
    let mut add_refs: Vec<serde_json::Value> = Vec::new();
    add_refs.extend(
        loaded
            .manifest
            .schemas
            .iter()
            .map(|r| serde_json::json!({"kind":"defschema","name":r.name,"hash":r.hash.as_str()})),
    );
    add_refs.extend(
        loaded
            .manifest
            .modules
            .iter()
            .map(|r| serde_json::json!({"kind":"defmodule","name":r.name,"hash":r.hash.as_str()})),
    );
    add_refs.extend(
        loaded
            .manifest
            .plans
            .iter()
            .map(|r| serde_json::json!({"kind":"defplan","name":r.name,"hash":r.hash.as_str()})),
    );
    add_refs.extend(
        loaded
            .manifest
            .caps
            .iter()
            .map(|r| serde_json::json!({"kind":"defcap","name":r.name,"hash":r.hash.as_str()})),
    );
    add_refs.extend(
        loaded
            .manifest
            .policies
            .iter()
            .map(|r| serde_json::json!({"kind":"defpolicy","name":r.name,"hash":r.hash.as_str()})),
    );
    add_refs.extend(
        loaded
            .manifest
            .effects
            .iter()
            .map(|r| serde_json::json!({"kind":"defeffect","name":r.name,"hash":r.hash.as_str()})),
    );
    add_refs.extend(
        loaded
            .manifest
            .secrets
            .iter()
            .filter_map(|e| match e {
                aos_air_types::SecretEntry::Ref(nr) => Some(nr),
                _ => None,
            })
            .map(|r| serde_json::json!({"kind":"defsecret","name":r.name,"hash":r.hash.as_str()})),
    );

    if !add_refs.is_empty() {
        patches.push(serde_json::json!({
            "set_manifest_refs": { "add": add_refs, "remove": [] }
        }));
    }

    if let Some(defaults) = &loaded.manifest.defaults {
        patches.push(serde_json::json!({
            "set_defaults": {
                "policy": defaults.policy,
                "cap_grants": defaults.cap_grants
            }
        }));
    }

    // routing.events
    let base_events = base_manifest
        .routing
        .as_ref()
        .map(|r| &r.events)
        .cloned()
        .unwrap_or_default();
    let pre = Hash::of_cbor(&base_events)
        .context("hash base routing.events")?
        .to_hex();
    let new_events = loaded
        .manifest
        .routing
        .as_ref()
        .map(|r| &r.events)
        .cloned()
        .unwrap_or_default();
    patches.push(serde_json::json!({
        "set_routing_events": { "pre_hash": pre, "events": new_events }
    }));

    // routing.inboxes
    let base_inboxes = base_manifest
        .routing
        .as_ref()
        .map(|r| &r.inboxes)
        .cloned()
        .unwrap_or_default();
    let pre = Hash::of_cbor(&base_inboxes)
        .context("hash base routing.inboxes")?
        .to_hex();
    let new_inboxes = loaded
        .manifest
        .routing
        .as_ref()
        .map(|r| &r.inboxes)
        .cloned()
        .unwrap_or_default();
    patches.push(serde_json::json!({
        "set_routing_inboxes": { "pre_hash": pre, "inboxes": new_inboxes }
    }));

    // triggers
    let pre = Hash::of_cbor(&base_manifest.triggers)
        .context("hash base triggers")?
        .to_hex();
    patches.push(serde_json::json!({
        "set_triggers": { "pre_hash": pre, "triggers": loaded.manifest.triggers }
    }));

    // module_bindings
    let pre = Hash::of_cbor(&base_manifest.module_bindings)
        .context("hash base module_bindings")?
        .to_hex();
    patches.push(serde_json::json!({
        "set_module_bindings": { "pre_hash": pre, "bindings": loaded.manifest.module_bindings }
    }));

    // secrets block
    let pre = Hash::of_cbor(&base_manifest.secrets)
        .context("hash base secrets")?
        .to_hex();
    patches.push(serde_json::json!({
        "set_secrets": { "pre_hash": pre, "secrets": loaded.manifest.secrets }
    }));

    Ok(serde_json::json!({
        "version": "1",
        "base_manifest_hash": base_manifest_hash,
        "patches": patches,
    }))
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
