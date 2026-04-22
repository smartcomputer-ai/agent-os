use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use aos_air_types::{AirNode, Manifest, ModuleRuntime, WasmArtifact};
use aos_authoring::bundle::import_genesis;
use aos_authoring::{
    SyncConfig, WorldBundle, build_bundle_from_local_world_ephemeral, build_patch_document,
    load_all_sync_secret_values, load_sync_config,
};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_kernel::Store;
use aos_sys::{WorkspaceCommit, WorkspaceCommitMeta};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde_json::Value;
use walkdir::WalkDir;

use crate::client::ApiClient;
use crate::commands::common::{encode_path_segment, universe_id_for_world};
use crate::workspace::{
    WorkspaceRef, decode_relative_path, encode_relative_path, join_workspace_path,
};

const LIST_LIMIT: u64 = 1_000;
const MODE_FILE_DEFAULT: u64 = 0o100644;
const MODE_FILE_EXEC: u64 = 0o100755;
const WORKSPACE_EVENT: &str = "sys/WorkspaceCommit@1";

fn with_universe(path: &str, universe_id: &str) -> String {
    format!("{path}?universe_id={universe_id}")
}

#[derive(Debug, Clone)]
pub struct LocalWorldDirs {
    pub root: PathBuf,
    pub air_dir: PathBuf,
    pub workflow_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct UploadedBundle {
    pub manifest_hash: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RemoteManifest {
    pub manifest_hash: String,
    pub manifest: Manifest,
}

#[derive(Debug, Clone)]
pub struct SyncEntry {
    pub reference: String,
    pub dir: PathBuf,
    pub ignore: Vec<String>,
    pub annotations: BTreeMap<String, BTreeMap<String, Value>>,
}

#[derive(Debug, Clone)]
struct LocalFile {
    mode: u64,
    bytes: Vec<u8>,
    hash: String,
}

#[derive(Debug, Clone)]
struct RemoteFile {
    hash: String,
    mode: u64,
}

#[derive(Debug, Clone)]
struct AnnotationTarget {
    path: Option<String>,
    patch: BTreeMap<String, Option<String>>,
}

pub fn resolve_local_dirs(local_root: Option<&Path>) -> Result<LocalWorldDirs> {
    let root = match local_root {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir().context("get current directory")?,
    };
    Ok(LocalWorldDirs {
        air_dir: root.join("air"),
        workflow_dir: root.join("workflow"),
        root,
    })
}

pub fn load_sync_entries(
    world_root: &Path,
    map: Option<&Path>,
    reference: Option<&str>,
    dir: Option<&Path>,
) -> Result<(PathBuf, SyncConfig, Vec<SyncEntry>)> {
    let (map_path, config) = load_sync_config(world_root, map)?;
    let map_root = map_path.parent().unwrap_or(world_root);
    let entries = match (reference, dir) {
        (Some(reference), Some(dir)) => vec![SyncEntry {
            reference: reference.to_string(),
            dir: resolve_map_path(world_root, dir),
            ignore: Vec::new(),
            annotations: BTreeMap::new(),
        }],
        (None, None) => config
            .workspaces
            .iter()
            .map(|entry| SyncEntry {
                reference: entry.reference.clone(),
                dir: resolve_map_path(map_root, &entry.dir),
                ignore: entry.ignore.clone(),
                annotations: entry.annotations.clone(),
            })
            .collect(),
        _ => {
            return Err(anyhow!(
                "workspace sync requires both <dir> and <ref> when specified explicitly"
            ));
        }
    };
    Ok((map_path, config, entries))
}

pub fn build_bundle_from_world(
    local_root: Option<&Path>,
    force_build: bool,
) -> Result<(aos_kernel::MemStore, WorldBundle, Vec<String>)> {
    let dirs = resolve_local_dirs(local_root)?;
    build_bundle_from_local_world_ephemeral(&dirs.root, force_build)
}

pub async fn upload_bundle(
    client: &ApiClient,
    store: &impl Store,
    bundle: &WorldBundle,
    mut warnings: Vec<String>,
    source_dirs: Option<&LocalWorldDirs>,
) -> Result<UploadedBundle> {
    let genesis = import_genesis(store, bundle).context("prepare genesis manifest for upload")?;
    for schema in &bundle.schemas {
        client.log(format!("uploading schema {}", schema.name));
        upload_node(client, &AirNode::Defschema(schema.clone()))
            .await
            .with_context(|| format!("upload schema {} to CAS", schema.name))?;
    }
    for module in &bundle.modules {
        client.log(format!("uploading module definition {}", module.name));
        upload_node(client, &AirNode::Defmodule(module.clone()))
            .await
            .with_context(|| format!("upload module definition {} to CAS", module.name))?;
        let ModuleRuntime::Wasm {
            artifact: WasmArtifact::WasmModule { hash },
        } = &module.runtime
        else {
            continue;
        };
        let wasm_hash = Hash::from_hex_str(hash.as_str())
            .with_context(|| format!("parse module wasm hash for {}", module.name))?;
        let bytes = store
            .get_blob(wasm_hash)
            .with_context(|| format!("load wasm blob for {}", module.name))?;
        let source_hint = module_source_hint(module.name.as_str(), source_dirs);
        client.log(format!(
            "uploading wasm blob for module {} from {} ({} bytes, sha256 {})",
            module.name,
            source_hint,
            bytes.len(),
            wasm_hash.to_hex()
        ));
        client
            .put_bytes(&format!("/v1/cas/blobs/{}", wasm_hash.to_hex()), bytes)
            .await
            .with_context(|| {
                format!(
                    "upload wasm blob for module {} from {} to /v1/cas/blobs/{}",
                    module.name,
                    source_hint,
                    wasm_hash.to_hex()
                )
            })?;
    }
    for workflow in &bundle.workflows {
        client.log(format!("uploading workflow {}", workflow.name));
        upload_node(client, &AirNode::Defworkflow(workflow.clone()))
            .await
            .with_context(|| format!("upload workflow {} to CAS", workflow.name))?;
    }
    for effect in &bundle.effects {
        client.log(format!("uploading effect {}", effect.name));
        upload_node(client, &AirNode::Defeffect(effect.clone()))
            .await
            .with_context(|| format!("upload effect {} to CAS", effect.name))?;
    }
    for secret in &bundle.secrets {
        client.log(format!("uploading secret {}", secret.name));
        upload_node(client, &AirNode::Defsecret(secret.clone()))
            .await
            .with_context(|| format!("upload secret {} to CAS", secret.name))?;
    }
    client.log("uploading manifest");
    let _ = upload_blob(client, &genesis.manifest_bytes)
        .await
        .context("upload genesis manifest to CAS")?;
    let manifest_hash = genesis.manifest_hash;
    if bundle.modules.is_empty() {
        warnings.push("manifest contains no modules".into());
    }
    Ok(UploadedBundle {
        manifest_hash,
        warnings,
    })
}

pub async fn sync_node_secrets(
    client: &ApiClient,
    universe_id: Option<&str>,
    local_root: Option<&Path>,
    map: Option<&Path>,
    actor: Option<&str>,
) -> Result<Value> {
    let dirs = resolve_local_dirs(local_root)?;
    let (map_path, _config, values) = load_all_sync_secret_values(&dirs.root, map)?;
    let mut synced = Vec::new();
    let mut unchanged = Vec::new();

    for value in values {
        let digest = Hash::of_bytes(&value.plaintext).to_hex();
        client.log(format!(
            "syncing node secret binding {} from {}:{}",
            value.binding, value.source, value.key
        ));
        let binding = client
            .put_json(
                &with_optional_universe(
                    &format!(
                        "/v1/secrets/bindings/{}",
                        encode_path_segment(&value.binding)
                    ),
                    universe_id,
                ),
                &node_secret_binding_body(actor),
            )
            .await
            .with_context(|| format!("upsert node secret binding '{}'", value.binding))?;
        let latest_version = binding.get("latest_version").and_then(Value::as_u64);
        if let Some(version) = latest_version {
            let existing = client
                .get_json(
                    &with_optional_universe(
                        &format!(
                            "/v1/secrets/bindings/{}/versions/{}",
                            encode_path_segment(&value.binding),
                            version
                        ),
                        universe_id,
                    ),
                    &[],
                )
                .await
                .with_context(|| {
                    format!("fetch node secret version '{}@{}'", value.binding, version)
                })?;
            if existing.get("digest").and_then(Value::as_str) == Some(digest.as_str()) {
                unchanged.push(serde_json::json!({
                    "binding": value.binding,
                    "digest": digest,
                    "version": version,
                    "source": value.source,
                    "key": value.key,
                }));
                continue;
            }
        }

        let put = client
            .post_json(
                &with_optional_universe(
                    &format!(
                        "/v1/secrets/bindings/{}/versions",
                        encode_path_segment(&value.binding)
                    ),
                    universe_id,
                ),
                &serde_json::json!({
                    "plaintext_b64": BASE64_STANDARD.encode(&value.plaintext),
                    "expected_digest": digest,
                    "actor": actor,
                }),
            )
            .await
            .with_context(|| format!("upload node secret value for '{}'", value.binding))?;
        synced.push(serde_json::json!({
            "binding": value.binding,
            "digest": put.get("digest").cloned().unwrap_or(Value::String(digest)),
            "version": put.get("version").cloned().unwrap_or(Value::Null),
            "source": value.source,
            "key": value.key,
        }));
    }

    Ok(serde_json::json!({
        "map": map_path.display().to_string(),
        "synced": synced,
        "unchanged": unchanged,
    }))
}

fn node_secret_binding_body(actor: Option<&str>) -> Value {
    serde_json::json!({
        "source_kind": "node_secret_store",
        "actor": actor,
    })
}

fn with_optional_universe(path: &str, universe_id: Option<&str>) -> String {
    match universe_id {
        Some(universe_id) => format!("{path}?universe_id={universe_id}"),
        None => path.to_string(),
    }
}

pub async fn fetch_remote_manifest(client: &ApiClient, world_id: &str) -> Result<RemoteManifest> {
    let response = client
        .get_json(&format!("/v1/worlds/{world_id}/manifest"), &[])
        .await?;
    let manifest: Manifest = serde_json::from_value(
        response
            .get("manifest")
            .cloned()
            .ok_or_else(|| anyhow!("manifest response missing manifest"))?,
    )
    .context("decode remote manifest")?;
    let manifest_hash = response
        .get("manifest_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("manifest response missing manifest_hash"))?;
    Ok(RemoteManifest {
        manifest_hash: manifest_hash.to_string(),
        manifest,
    })
}

pub fn build_patch(remote: &RemoteManifest, bundle: &WorldBundle) -> Result<Value> {
    serde_json::to_value(build_patch_document(
        bundle,
        &remote.manifest,
        &remote.manifest_hash,
    )?)
    .context("encode patch document")
}

pub async fn upload_patch_json(
    client: &ApiClient,
    world_id: &str,
    patch: &Value,
) -> Result<String> {
    let bytes = serde_json::to_vec(patch).context("encode patch document json")?;
    upload_blob_for_world(client, world_id, &bytes).await
}

pub async fn upload_patch_bytes(
    client: &ApiClient,
    world_id: &str,
    bytes: &[u8],
) -> Result<String> {
    upload_blob_for_world(client, world_id, bytes).await
}

pub async fn sync_workspace_push(
    client: &ApiClient,
    world_id: &str,
    entry: &SyncEntry,
    prune: bool,
    message: Option<&str>,
) -> Result<Value> {
    let parsed = crate::workspace::parse_workspace_ref(&entry.reference)?;
    if parsed.version.is_some() {
        return Err(anyhow!(
            "push ref cannot include a version: {}",
            entry.reference
        ));
    }
    let local = collect_local_files(&entry.dir, &entry.ignore, parsed.path.as_deref())?;
    let resolved = resolve_workspace(client, world_id, &parsed).await?;
    let universe_id = universe_id_for_world(client, world_id).await?;
    let base_root = if let Some(root) = resolved.root_hash.clone() {
        root
    } else {
        client
            .post_json(
                &with_universe("/v1/workspace/roots", &universe_id),
                &Value::Null,
            )
            .await?
            .get("root_hash")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("empty root response missing root_hash"))?
            .to_string()
    };
    let remote = list_remote_files(client, world_id, &base_root, parsed.path.as_deref()).await?;
    let mut operations = Vec::new();

    for (path, file) in &local {
        let needs_write = match remote.get(path) {
            Some(existing) => existing.hash != file.hash || existing.mode != file.mode,
            None => true,
        };
        if needs_write {
            operations.push(serde_json::json!({
                "op": "write_bytes",
                "path": path,
                "bytes_b64": BASE64_STANDARD.encode(&file.bytes),
                "mode": file.mode,
            }));
        }
    }

    if prune {
        for path in remote.keys() {
            if !local.contains_key(path) {
                operations.push(serde_json::json!({
                    "op": "remove",
                    "path": path,
                }));
            }
        }
    }

    let annotation_targets =
        build_annotation_targets(client, world_id, &entry.annotations, parsed.path.as_deref())
            .await?;
    for target in annotation_targets {
        operations.push(serde_json::json!({
            "op": "set_annotations",
            "path": target.path,
            "annotations_patch": target.patch,
        }));
    }
    if let Some(message) = message {
        let hash = upload_blob_for_world(client, world_id, message.as_bytes()).await?;
        operations.push(serde_json::json!({
            "op": "set_annotations",
            "path": parsed.path,
            "annotations_patch": {
                "sys/commit.message": hash,
            },
        }));
    }

    let new_root = if operations.is_empty() && resolved.root_hash.is_some() {
        base_root.clone()
    } else {
        client
            .post_json(
                &with_universe(
                    &format!("/v1/workspace/roots/{base_root}/apply"),
                    &universe_id,
                ),
                &serde_json::json!({ "operations": operations }),
            )
            .await?
            .get("new_root_hash")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("workspace apply response missing new_root_hash"))?
            .to_string()
    };

    let event = WorkspaceCommit {
        workspace: parsed.workspace,
        expected_head: resolved.head,
        meta: WorkspaceCommitMeta {
            root_hash: new_root.clone(),
            owner: resolve_owner(),
            created_at: now_ns(),
        },
    };
    let payload = serde_cbor::to_vec(&event).context("encode workspace commit event")?;
    let response = client
        .post_json(
            &format!("/v1/worlds/{world_id}/events"),
            &serde_json::json!({
                "schema": WORKSPACE_EVENT,
                "value_b64": BASE64_STANDARD.encode(payload),
            }),
        )
        .await?;
    Ok(serde_json::json!({
        "workspace": entry.reference,
        "base_root_hash": base_root,
        "new_root_hash": new_root,
        "commit": response,
    }))
}

pub async fn sync_workspace_pull(
    client: &ApiClient,
    world_id: &str,
    entry: &SyncEntry,
    prune: bool,
) -> Result<Value> {
    let parsed = crate::workspace::parse_workspace_ref(&entry.reference)?;
    let resolved = resolve_workspace(client, world_id, &parsed).await?;
    let root_hash = resolved
        .root_hash
        .ok_or_else(|| anyhow!("workspace '{}' does not exist", entry.reference))?;
    let remote = list_remote_files(client, world_id, &root_hash, parsed.path.as_deref()).await?;
    let universe_id = universe_id_for_world(client, world_id).await?;
    fs::create_dir_all(&entry.dir)
        .with_context(|| format!("create pull directory {}", entry.dir.display()))?;

    let mut pulled = Vec::new();
    for path in remote.keys() {
        let rel = decode_relative_path(path);
        let dest = entry.dir.join(&rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create pull directory {}", parent.display()))?;
        }
        let bytes = client
            .get_bytes(&format!("/v1/workspace/roots/{root_hash}/bytes"), &{
                let query = vec![
                    ("universe_id", Some(universe_id.clone())),
                    ("path", Some(path.clone())),
                ];
                query
            })
            .await?;
        fs::write(&dest, bytes).with_context(|| format!("write {}", dest.display()))?;
        pulled.push(dest.display().to_string());
    }

    if prune {
        prune_local_dir(
            &entry.dir,
            remote
                .keys()
                .map(|path| decode_relative_path(path))
                .collect(),
        )?;
    }

    Ok(serde_json::json!({
        "workspace": entry.reference,
        "root_hash": root_hash,
        "files": pulled,
    }))
}

#[derive(Debug, Clone)]
struct WorkspaceResolution {
    head: Option<u64>,
    root_hash: Option<String>,
}

async fn resolve_workspace(
    client: &ApiClient,
    world_id: &str,
    reference: &WorkspaceRef,
) -> Result<WorkspaceResolution> {
    let response = client
        .get_json(
            &format!("/v1/worlds/{world_id}/workspace/resolve"),
            &[
                ("workspace", Some(reference.workspace.clone())),
                ("version", reference.version.map(|value| value.to_string())),
            ],
        )
        .await?;
    Ok(WorkspaceResolution {
        head: response.get("head").and_then(Value::as_u64),
        root_hash: response
            .get("root_hash")
            .and_then(Value::as_str)
            .map(ToString::to_string),
    })
}

async fn list_remote_files(
    client: &ApiClient,
    world_id: &str,
    root_hash: &str,
    base_path: Option<&str>,
) -> Result<HashMap<String, RemoteFile>> {
    let universe_id = universe_id_for_world(client, world_id).await?;
    let mut cursor = None;
    let mut out = HashMap::new();
    loop {
        let response = client
            .get_json(
                &format!("/v1/workspace/roots/{root_hash}/entries"),
                &[
                    ("universe_id", Some(universe_id.clone())),
                    ("path", base_path.map(ToString::to_string)),
                    ("scope", Some("subtree".into())),
                    ("cursor", cursor.clone()),
                    ("limit", Some(LIST_LIMIT.to_string())),
                ],
            )
            .await?;
        let entries = response
            .get("entries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for entry in entries {
            if entry.get("kind").and_then(Value::as_str) != Some("file") {
                continue;
            }
            let path = entry
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("workspace entry missing path"))?;
            let hash = entry
                .get("hash")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("workspace entry missing hash"))?;
            let mode = entry
                .get("mode")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("workspace entry missing mode"))?;
            out.insert(
                path.to_string(),
                RemoteFile {
                    hash: hash.to_string(),
                    mode: normalize_file_mode(mode)?,
                },
            );
        }
        cursor = response
            .get("next_cursor")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if cursor.is_none() {
            break;
        }
    }
    Ok(out)
}

fn collect_local_files(
    root: &Path,
    ignore_rules: &[String],
    base_path: Option<&str>,
) -> Result<HashMap<String, LocalFile>> {
    let matcher = IgnoreMatcher::new(root, ignore_rules)?;
    let prefix = base_path.map(PathBuf::from).unwrap_or_default();
    let source_root = root.join(&prefix);
    if !source_root.exists() {
        return Err(anyhow!("local path '{}' not found", source_root.display()));
    }

    let mut out = HashMap::new();
    for entry in WalkDir::new(&source_root)
        .into_iter()
        .filter_map(Result::ok)
    {
        let file_type = entry.file_type();
        let rel = entry
            .path()
            .strip_prefix(root)
            .with_context(|| format!("strip prefix {}", root.display()))?;
        if file_type.is_dir() {
            if matcher.is_ignored(rel, true) {
                continue;
            }
            continue;
        }
        if !file_type.is_file() || matcher.is_ignored(rel, false) {
            continue;
        }
        let rel_encoded = encode_relative_path(rel)?;
        let bytes =
            fs::read(entry.path()).with_context(|| format!("read {}", entry.path().display()))?;
        let mode = if is_executable(entry.path()) {
            MODE_FILE_EXEC
        } else {
            MODE_FILE_DEFAULT
        };
        out.insert(
            rel_encoded.clone(),
            LocalFile {
                mode,
                hash: Hash::of_bytes(&bytes).to_hex(),
                bytes,
            },
        );
    }
    Ok(out)
}

async fn build_annotation_targets(
    client: &ApiClient,
    world_id: &str,
    annotations: &BTreeMap<String, BTreeMap<String, Value>>,
    base_path: Option<&str>,
) -> Result<Vec<AnnotationTarget>> {
    let mut out = Vec::new();
    for (path, entries) in annotations {
        let target_path = if path.trim().is_empty() {
            base_path.map(ToString::to_string)
        } else {
            let encoded = encode_relative_path(Path::new(path))?;
            Some(join_workspace_path(base_path, &encoded))
        };
        let mut patch = BTreeMap::new();
        for (key, value) in entries {
            let bytes = match value {
                Value::String(text) => text.as_bytes().to_vec(),
                other => to_canonical_cbor(other).context("encode annotation value")?,
            };
            let hash = upload_blob_for_world(client, world_id, &bytes).await?;
            patch.insert(key.clone(), Some(hash));
        }
        if !patch.is_empty() {
            out.push(AnnotationTarget {
                path: target_path,
                patch,
            });
        }
    }
    Ok(out)
}

async fn upload_blob(client: &ApiClient, bytes: &[u8]) -> Result<String> {
    let hash = Hash::of_bytes(bytes).to_hex();
    client
        .put_bytes(&format!("/v1/cas/blobs/{hash}"), bytes.to_vec())
        .await?;
    Ok(hash)
}

async fn upload_blob_for_world(client: &ApiClient, world_id: &str, bytes: &[u8]) -> Result<String> {
    let hash = Hash::of_bytes(bytes).to_hex();
    let universe_id = universe_id_for_world(client, world_id).await?;
    let path = with_universe(&format!("/v1/cas/blobs/{hash}"), &universe_id);
    if client.head_exists(&path).await? {
        return Ok(hash);
    }
    client.put_bytes(&path, bytes.to_vec()).await?;
    Ok(hash)
}

async fn upload_node<T: serde::Serialize>(client: &ApiClient, value: &T) -> Result<()> {
    let bytes = to_canonical_cbor(value).context("encode node to canonical cbor")?;
    let _ = upload_blob(client, &bytes).await?;
    Ok(())
}

fn module_source_hint(module_name: &str, source_dirs: Option<&LocalWorldDirs>) -> String {
    let Some(source_dirs) = source_dirs else {
        return "local build inputs".to_string();
    };
    let workflow_dir = source_dirs.workflow_dir.display();
    let modules_dir = source_dirs.root.join("modules");
    if source_dirs.workflow_dir.exists() {
        format!(
            "workflow dir {} via ephemeral build artifact (fallback modules dir {} for content-addressed wasm named {}-<sha256>.wasm)",
            workflow_dir,
            modules_dir.display(),
            module_name
        )
    } else {
        format!(
            "modules dir {} (content-addressed wasm named {}-<sha256>.wasm)",
            modules_dir.display(),
            module_name
        )
    }
}

fn resolve_map_path(root: &Path, path: &Path) -> PathBuf {
    if path.is_relative() {
        root.join(path)
    } else {
        path.to_path_buf()
    }
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            return meta.permissions().mode() & 0o111 != 0;
        }
    }
    false
}

fn normalize_file_mode(mode: u64) -> Result<u64> {
    match mode {
        0o644 | MODE_FILE_DEFAULT => Ok(MODE_FILE_DEFAULT),
        0o755 | MODE_FILE_EXEC => Ok(MODE_FILE_EXEC),
        _ => Err(anyhow!(
            "invalid workspace file mode {mode:o}; expected 644/755 or 100644/100755"
        )),
    }
}

fn now_ns() -> u64 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    duration
        .as_secs()
        .saturating_mul(1_000_000_000)
        .saturating_add(duration.subsec_nanos() as u64)
}

fn resolve_owner() -> String {
    if let Ok(value) = std::env::var("AOS_OWNER") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Ok(value) = std::env::var("USER") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    "aos".into()
}

fn prune_local_dir(root: &Path, keep: Vec<PathBuf>) -> Result<()> {
    let keep: std::collections::HashSet<PathBuf> = keep.into_iter().collect();
    for entry in WalkDir::new(root)
        .contents_first(true)
        .into_iter()
        .filter_map(Result::ok)
    {
        let rel = entry
            .path()
            .strip_prefix(root)
            .with_context(|| format!("strip prefix {}", root.display()))?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        if keep.contains(rel) {
            continue;
        }
        if entry.file_type().is_file() {
            fs::remove_file(entry.path())
                .with_context(|| format!("remove {}", entry.path().display()))?;
        } else if entry.file_type().is_dir() && entry.path().read_dir()?.next().is_none() {
            fs::remove_dir(entry.path())
                .with_context(|| format!("remove {}", entry.path().display()))?;
        }
    }
    Ok(())
}

struct IgnoreMatcher {
    gitignore: Gitignore,
}

impl IgnoreMatcher {
    fn new(root: &Path, rules: &[String]) -> Result<Self> {
        let mut builder = GitignoreBuilder::new(root);
        for rule in rules {
            builder
                .add_line(None, rule)
                .with_context(|| format!("invalid ignore rule '{rule}'"))?;
        }
        let gitignore = builder.build().context("build ignore matcher")?;
        Ok(Self { gitignore })
    }

    fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        self.gitignore.matched(path, is_dir).is_ignore()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_secret_binding_body_uses_node_secret_store() {
        let body = node_secret_binding_body(Some("sync"));
        assert_eq!(
            body.get("source_kind").and_then(Value::as_str),
            Some("node_secret_store")
        );
        assert_eq!(body.get("actor").and_then(Value::as_str), Some("sync"));
    }
}
