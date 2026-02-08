//! Workspace filesystem sync helpers for `aos push`/`aos pull`.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use aos_cbor::Hash;
use aos_effects::{EffectKind, IntentBuilder, ReceiptStatus};
use aos_host::host::WorldHost;
use aos_store::{FsStore, Store};
use aos_sys::{WorkspaceCommit, WorkspaceCommitMeta};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use walkdir::{DirEntry, WalkDir};

const WORKSPACE_CAP: &str = "sys/workspace@1";
const WORKSPACE_EVENT: &str = "sys/WorkspaceCommit@1";
const LIST_LIMIT: u64 = 1000;
const MODE_FILE_DEFAULT: u64 = 0o644;
const MODE_FILE_EXEC: u64 = 0o755;

#[derive(Debug)]
pub struct SyncPushOptions<'a> {
    pub prune: bool,
    pub message: Option<&'a str>,
}

#[derive(Debug)]
pub struct SyncPullOptions {
    pub prune: bool,
    pub dry_run: bool,
}

#[derive(Debug, Default)]
pub struct SyncStats {
    pub writes: usize,
    pub removes: usize,
    pub annotations: usize,
    pub committed: bool,
}

#[derive(Debug)]
struct WorkspaceRefParts {
    workspace: String,
    version: Option<u64>,
    path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceResolveParams {
    workspace: String,
    version: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceResolveReceipt {
    exists: bool,
    resolved_version: Option<u64>,
    head: Option<u64>,
    root_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceEmptyRootParams {
    workspace: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceEmptyRootReceipt {
    root_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceListParams {
    root_hash: String,
    path: Option<String>,
    scope: Option<String>,
    cursor: Option<String>,
    limit: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceListEntry {
    path: String,
    kind: String,
    hash: Option<String>,
    size: Option<u64>,
    mode: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceListReceipt {
    entries: Vec<WorkspaceListEntry>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceReadBytesParams {
    root_hash: String,
    path: String,
    range: Option<WorkspaceReadBytesRange>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceReadBytesRange {
    start: u64,
    end: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceWriteBytesParams {
    root_hash: String,
    path: String,
    bytes: Vec<u8>,
    mode: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceWriteBytesReceipt {
    new_root_hash: String,
    blob_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceRemoveParams {
    root_hash: String,
    path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceRemoveReceipt {
    new_root_hash: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(transparent)]
struct WorkspaceAnnotationsPatch(BTreeMap<String, Option<String>>);

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsSetParams {
    root_hash: String,
    path: Option<String>,
    annotations_patch: WorkspaceAnnotationsPatch,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsSetReceipt {
    new_root_hash: String,
    annotations_hash: String,
}

#[derive(Debug)]
struct IgnoreMatcher {
    gitignore: Gitignore,
    config: Gitignore,
    prefix: PathBuf,
}

#[derive(Debug, Clone)]
struct LocalFileEntry {
    path: PathBuf,
    hash: String,
    mode: u64,
}

#[derive(Debug)]
struct RemoteFileEntry {
    hash: String,
    mode: u64,
}

pub fn sync_workspace_push(
    host: &mut WorldHost<FsStore>,
    store: &FsStore,
    reference: &str,
    dir: &Path,
    ignore: &[String],
    annotations: &BTreeMap<String, BTreeMap<String, JsonValue>>,
    opts: &SyncPushOptions<'_>,
) -> Result<SyncStats> {
    let mut stats = SyncStats::default();
    let parsed = parse_workspace_ref(reference)?;
    if parsed.version.is_some() {
        anyhow::bail!("push ref cannot include a version: {}", reference);
    }
    ensure_dir_exists(dir)?;
    let matcher = IgnoreMatcher::new(dir, ignore)?;
    let local = collect_local_files(dir, &matcher, parsed.path.as_deref())?;
    let (mut root_hash, expected_head, existed) =
        resolve_workspace_for_sync(host, &parsed.workspace)?;
    let remote = list_workspace_files(host, &root_hash, parsed.path.as_deref())?;

    let mut writes: Vec<(String, LocalFileEntry)> = Vec::new();
    for (path, entry) in &local {
        let needs_write = match remote.get(path) {
            Some(remote_entry) => {
                remote_entry.hash != entry.hash || remote_entry.mode != entry.mode
            }
            None => true,
        };
        if needs_write {
            writes.push((path.clone(), entry.clone()));
        }
    }

    let mut removes: Vec<String> = Vec::new();
    if opts.prune {
        for path in remote.keys() {
            if local.contains_key(path) {
                continue;
            }
            if should_skip_prune(path, parsed.path.as_deref(), &matcher)? {
                continue;
            }
            removes.push(path.clone());
        }
    }

    let mut annotation_map = annotations.clone();
    if let Some(message) = opts.message {
        annotation_map.entry(String::new()).or_default().insert(
            "sys/commit.message".to_string(),
            JsonValue::String(message.to_string()),
        );
    }
    let annotation_targets =
        build_annotation_targets(store, &annotation_map, parsed.path.as_deref())?;

    if writes.is_empty() && removes.is_empty() && annotation_targets.is_empty() && existed {
        return Ok(stats);
    }

    writes.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, entry) in writes {
        let bytes =
            fs::read(&entry.path).with_context(|| format!("read file {}", entry.path.display()))?;
        let receipt = workspace_write_bytes(
            host,
            &WorkspaceWriteBytesParams {
                root_hash: root_hash.clone(),
                path: path.clone(),
                bytes,
                mode: Some(entry.mode),
            },
        )?;
        root_hash = receipt.new_root_hash;
        stats.writes += 1;
    }

    removes.sort();
    for path in removes {
        let receipt = workspace_remove(
            host,
            &WorkspaceRemoveParams {
                root_hash: root_hash.clone(),
                path,
            },
        )?;
        root_hash = receipt.new_root_hash;
        stats.removes += 1;
    }

    for target in annotation_targets {
        let receipt = workspace_annotations_set(
            host,
            &WorkspaceAnnotationsSetParams {
                root_hash: root_hash.clone(),
                path: target.path,
                annotations_patch: WorkspaceAnnotationsPatch(target.patch),
            },
        )?;
        root_hash = receipt.new_root_hash;
        stats.annotations += 1;
    }

    let owner = resolve_owner();
    commit_workspace(host, &parsed.workspace, expected_head, &root_hash, &owner)?;
    stats.committed = true;
    Ok(stats)
}

fn should_skip_prune(
    full_path: &str,
    base_path: Option<&str>,
    matcher: &IgnoreMatcher,
) -> Result<bool> {
    let rel = strip_base_path(full_path, base_path)?;
    let rel_path = decode_relative_path(&rel)?;
    Ok(matcher.is_ignored(&rel_path, false))
}

pub fn sync_workspace_pull(
    host: &mut WorldHost<FsStore>,
    reference: &str,
    dir: &Path,
    ignore: &[String],
    opts: &SyncPullOptions,
) -> Result<SyncStats> {
    let mut stats = SyncStats::default();
    let parsed = parse_workspace_ref(reference)?;
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let matcher = IgnoreMatcher::new(dir, ignore)?;
    let resolved = workspace_resolve(
        host,
        &WorkspaceResolveParams {
            workspace: parsed.workspace.clone(),
            version: parsed.version,
        },
    )?;
    if !resolved.exists {
        anyhow::bail!("workspace '{}' not found", parsed.workspace);
    }
    let root_hash = resolved
        .root_hash
        .clone()
        .ok_or_else(|| anyhow!("workspace root hash missing"))?;
    let remote = list_workspace_files(host, &root_hash, parsed.path.as_deref())?;
    let decoded = decode_workspace_entries(&remote, parsed.path.as_deref(), &matcher)?;

    if !opts.dry_run {
        for (rel_path, remote_path, mode) in &decoded {
            let params = WorkspaceReadBytesParams {
                root_hash: root_hash.clone(),
                path: remote_path.clone(),
                range: None,
            };
            let bytes = workspace_read_bytes(host, &params)?;
            let out_path = dir.join(rel_path);
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            fs::write(&out_path, bytes).with_context(|| format!("write {}", out_path.display()))?;
            set_file_mode(&out_path, *mode)?;
            stats.writes += 1;
        }
    } else {
        stats.writes = decoded.len();
    }

    if opts.prune && !opts.dry_run {
        let local_paths = collect_local_paths(dir, &matcher)?;
        let remote_paths: HashSet<PathBuf> =
            decoded.iter().map(|(path, _, _)| path.clone()).collect();
        for path in local_paths {
            if !remote_paths.contains(&path) {
                let full = dir.join(&path);
                fs::remove_file(&full).with_context(|| format!("remove {}", full.display()))?;
                stats.removes += 1;
            }
        }
    }

    Ok(stats)
}

fn parse_workspace_ref(input: &str) -> Result<WorkspaceRefParts> {
    let input = input.trim_end_matches('/');
    if input.is_empty() {
        anyhow::bail!("workspace ref is required");
    }
    if input.starts_with('/') {
        anyhow::bail!("workspace ref cannot start with '/'");
    }
    let (head, path) = match input.split_once('/') {
        Some((head, path)) => {
            if path.is_empty() || path.starts_with('/') {
                anyhow::bail!("invalid workspace path");
            }
            (head, Some(path.to_string()))
        }
        None => (input, None),
    };
    let (workspace, version) = match head.split_once('@') {
        Some((name, version)) => {
            if name.is_empty() || version.is_empty() {
                anyhow::bail!("invalid workspace ref");
            }
            let version = version
                .parse::<u64>()
                .map_err(|_| anyhow!("invalid workspace version"))?;
            (name.to_string(), Some(version))
        }
        None => (head.to_string(), None),
    };
    Ok(WorkspaceRefParts {
        workspace,
        version,
        path,
    })
}

fn ensure_dir_exists(dir: &Path) -> Result<()> {
    if !dir.exists() {
        anyhow::bail!("local dir does not exist: {}", dir.display());
    }
    if !dir.is_dir() {
        anyhow::bail!("local path is not a directory: {}", dir.display());
    }
    Ok(())
}

fn collect_local_files(
    root: &Path,
    matcher: &IgnoreMatcher,
    base_path: Option<&str>,
) -> Result<BTreeMap<String, LocalFileEntry>> {
    let mut files = BTreeMap::new();
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| filter_entry(root, matcher, entry));

    for entry in walker {
        let entry = entry?;
        if entry.path() == root {
            continue;
        }
        let file_type = entry.file_type();
        if file_type.is_dir() {
            continue;
        }
        if file_type.is_symlink() {
            anyhow::bail!("symlinks are not supported: {}", entry.path().display());
        }
        if !file_type.is_file() {
            anyhow::bail!(
                "unsupported file type in workspace sync: {}",
                entry.path().display()
            );
        }
        let rel = entry.path().strip_prefix(root).with_context(|| {
            format!(
                "strip prefix {} from {}",
                root.display(),
                entry.path().display()
            )
        })?;
        let rel_encoded = encode_relative_path(rel)?;
        let full_path = join_workspace_path(base_path, &rel_encoded);
        let bytes =
            fs::read(entry.path()).with_context(|| format!("read {}", entry.path().display()))?;
        let hash = Hash::of_bytes(&bytes).to_hex();
        let mode = file_mode_from_metadata(&entry.metadata()?)?;
        files.insert(
            full_path,
            LocalFileEntry {
                path: entry.path().to_path_buf(),
                hash,
                mode,
            },
        );
    }
    Ok(files)
}

fn collect_local_paths(root: &Path, matcher: &IgnoreMatcher) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| filter_entry(root, matcher, entry));
    for entry in walker {
        let entry = entry?;
        if entry.path() == root {
            continue;
        }
        let file_type = entry.file_type();
        if file_type.is_dir() {
            continue;
        }
        if file_type.is_symlink() {
            anyhow::bail!("symlinks are not supported: {}", entry.path().display());
        }
        if !file_type.is_file() {
            anyhow::bail!(
                "unsupported file type in workspace sync: {}",
                entry.path().display()
            );
        }
        let rel = entry.path().strip_prefix(root).with_context(|| {
            format!(
                "strip prefix {} from {}",
                root.display(),
                entry.path().display()
            )
        })?;
        let rel_path = rel.to_path_buf();
        paths.push(rel_path);
    }
    paths.sort();
    Ok(paths)
}

fn filter_entry(root: &Path, matcher: &IgnoreMatcher, entry: &DirEntry) -> bool {
    if entry.path() == root {
        return true;
    }
    if is_git_dir(entry) {
        return false;
    }
    let Ok(rel) = entry.path().strip_prefix(root) else {
        return true;
    };
    !matcher.is_ignored(rel, entry.file_type().is_dir())
}

fn list_workspace_files(
    host: &mut WorldHost<FsStore>,
    root_hash: &str,
    base_path: Option<&str>,
) -> Result<BTreeMap<String, RemoteFileEntry>> {
    let mut out = BTreeMap::new();
    let mut cursor = None;
    loop {
        let receipt = workspace_list(
            host,
            &WorkspaceListParams {
                root_hash: root_hash.to_string(),
                path: base_path.map(|s| s.to_string()),
                scope: Some("subtree".to_string()),
                cursor: cursor.clone(),
                limit: LIST_LIMIT,
            },
        )?;
        for entry in receipt.entries {
            if entry.kind != "file" {
                continue;
            }
            let Some(hash) = entry.hash else {
                anyhow::bail!("workspace entry missing hash: {}", entry.path);
            };
            let Some(mode) = entry.mode else {
                anyhow::bail!("workspace entry missing mode: {}", entry.path);
            };
            out.insert(entry.path, RemoteFileEntry { hash, mode });
        }
        match receipt.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    Ok(out)
}

struct AnnotationTarget {
    path: Option<String>,
    patch: BTreeMap<String, Option<String>>,
}

fn build_annotation_targets(
    store: &FsStore,
    annotations: &BTreeMap<String, BTreeMap<String, JsonValue>>,
    base_path: Option<&str>,
) -> Result<Vec<AnnotationTarget>> {
    let mut targets = Vec::new();
    for (path_key, values) in annotations {
        let target_path = resolve_annotation_path(path_key, base_path)?;
        let mut patch = BTreeMap::new();
        for (key, value) in values {
            let key = normalize_annotation_key(key)?;
            let bytes = match value {
                JsonValue::String(text) => text.as_bytes().to_vec(),
                _ => {
                    aos_cbor::to_canonical_cbor(value).context("encode annotation value to CBOR")?
                }
            };
            let hash = store.put_blob(&bytes).context("store annotation blob")?;
            patch.insert(key, Some(hash.to_hex()));
        }
        if !patch.is_empty() {
            targets.push(AnnotationTarget {
                path: target_path,
                patch,
            });
        }
    }
    Ok(targets)
}

fn resolve_annotation_path(path: &str, base_path: Option<&str>) -> Result<Option<String>> {
    if path.trim().is_empty() {
        return Ok(base_path.map(|s| s.to_string()));
    }
    let encoded = encode_relative_path(Path::new(path))?;
    Ok(Some(join_workspace_path(base_path, &encoded)))
}

fn decode_workspace_entries(
    remote: &BTreeMap<String, RemoteFileEntry>,
    base_path: Option<&str>,
    matcher: &IgnoreMatcher,
) -> Result<Vec<(PathBuf, String, u64)>> {
    let mut decoded = Vec::new();
    let mut seen = HashSet::new();
    for (full_path, entry) in remote {
        let rel = strip_base_path(full_path, base_path)?;
        let rel_path = decode_relative_path(&rel)?;
        if matcher.is_ignored(&rel_path, false) {
            continue;
        }
        if !seen.insert(rel_path.clone()) {
            anyhow::bail!("workspace path collision after decode: {}", rel);
        }
        decoded.push((rel_path, full_path.clone(), entry.mode));
    }
    decoded.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(decoded)
}

fn strip_base_path(path: &str, base_path: Option<&str>) -> Result<String> {
    let Some(base) = base_path else {
        return Ok(path.to_string());
    };
    if path == base {
        anyhow::bail!("workspace ref path must be a directory: {}", base);
    }
    let prefix = format!("{base}/");
    if !path.starts_with(&prefix) {
        anyhow::bail!("workspace path '{}' not under '{}'", path, base);
    }
    Ok(path[prefix.len()..].to_string())
}

fn join_workspace_path(base: Option<&str>, rel: &str) -> String {
    match base {
        Some(base) if !base.is_empty() && !rel.is_empty() => format!("{base}/{rel}"),
        Some(base) if !base.is_empty() => base.to_string(),
        _ => rel.to_string(),
    }
}

fn encode_relative_path(path: &Path) -> Result<String> {
    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                let raw = name
                    .to_str()
                    .ok_or_else(|| anyhow!("non-UTF-8 path segment"))?;
                segments.push(encode_segment(raw));
            }
            Component::CurDir => {}
            Component::ParentDir => {
                anyhow::bail!("parent path components are not allowed");
            }
            Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("absolute paths are not allowed");
            }
        }
    }
    if segments.is_empty() {
        anyhow::bail!("path is empty");
    }
    Ok(segments.join("/"))
}

fn decode_relative_path(path: &str) -> Result<PathBuf> {
    if path.is_empty() {
        anyhow::bail!("invalid workspace path");
    }
    let mut out = PathBuf::new();
    for seg in path.split('/') {
        if seg.is_empty() {
            anyhow::bail!("invalid workspace path segment");
        }
        let decoded = decode_segment(seg)?;
        if decoded == "." || decoded == ".." {
            anyhow::bail!("invalid workspace path segment");
        }
        out.push(decoded);
    }
    Ok(out)
}

fn encode_segment(raw: &str) -> String {
    if !raw.is_empty()
        && !raw.starts_with('~')
        && raw.chars().all(|c| {
            matches!(c, 'a'..='z'
            | 'A'..='Z'
            | '0'..='9'
            | '.'
            | '_'
            | '-'
            | '~')
        })
    {
        return raw.to_string();
    }
    let mut out = String::from("~");
    for byte in raw.as_bytes() {
        out.push_str(&format!("{:02X}", byte));
    }
    out
}

fn decode_segment(seg: &str) -> Result<String> {
    if !seg.starts_with('~') {
        return Ok(seg.to_string());
    }
    let hex = &seg[1..];
    if hex.is_empty() || hex.len() % 2 != 0 {
        anyhow::bail!("invalid ~-hex workspace segment");
    }
    let bytes = hex::decode(hex).context("decode ~-hex workspace segment")?;
    String::from_utf8(bytes).context("decode ~-hex workspace segment utf-8")
}

impl IgnoreMatcher {
    fn new(scope: &Path, patterns: &[String]) -> Result<Self> {
        let scope = scope.canonicalize().unwrap_or_else(|_| scope.to_path_buf());
        let root = find_git_root(&scope).unwrap_or_else(|| scope.to_path_buf());
        let prefix = scope
            .strip_prefix(&root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| PathBuf::new());
        let gitignore = build_gitignore(&root, &scope)?;
        let config = build_config_ignore(&root, &prefix, patterns)?;
        Ok(Self {
            gitignore,
            config,
            prefix,
        })
    }

    fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        let full_path = if self.prefix.as_os_str().is_empty() {
            path.to_path_buf()
        } else {
            self.prefix.join(path)
        };
        let config_match = self.config.matched_path_or_any_parents(&full_path, is_dir);
        if config_match.is_whitelist() {
            return false;
        }
        if config_match.is_ignore() {
            return true;
        }
        let git_match = self
            .gitignore
            .matched_path_or_any_parents(&full_path, is_dir);
        if git_match.is_whitelist() {
            return false;
        }
        git_match.is_ignore()
    }
}

fn is_git_dir(entry: &DirEntry) -> bool {
    entry
        .path()
        .components()
        .any(|component| component.as_os_str() == ".git")
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let git_path = dir.join(".git");
        if git_path.is_dir() || git_path.is_file() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

fn build_gitignore(root: &Path, scope: &Path) -> Result<Gitignore> {
    let mut builder = GitignoreBuilder::new(root);
    // Include root + ancestor .gitignore files relevant to `scope`.
    let mut current = Some(scope);
    while let Some(dir) = current {
        add_gitignore_if_exists(&mut builder, dir)?;
        if dir == root {
            break;
        }
        current = dir.parent();
    }
    // Include nested .gitignore files under `scope`.
    let walk = WalkDir::new(scope)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_git_dir(entry));
    for entry in walk {
        let entry = entry?;
        if entry.file_type().is_file() && entry.file_name() == ".gitignore" {
            add_gitignore_if_exists(&mut builder, entry.path())?;
        }
    }
    builder.build().context("build gitignore matcher")
}

fn add_gitignore_if_exists(builder: &mut GitignoreBuilder, path: &Path) -> Result<()> {
    let gitignore_path = if path.file_name().is_some_and(|name| name == ".gitignore") {
        path.to_path_buf()
    } else {
        path.join(".gitignore")
    };
    if !gitignore_path.is_file() {
        return Ok(());
    }
    if let Some(err) = builder.add(&gitignore_path) {
        return Err(anyhow!("parse {}: {err}", gitignore_path.display()));
    }
    Ok(())
}

fn build_config_ignore(root: &Path, prefix: &Path, patterns: &[String]) -> Result<Gitignore> {
    let mut builder = GitignoreBuilder::new(root);
    if patterns.is_empty() {
        return builder.build().context("build ignore matcher");
    }
    let prefix_str = path_to_slash(prefix)?;
    for pattern in patterns {
        let trimmed = pattern.trim();
        if trimmed.is_empty() {
            continue;
        }
        let line = if prefix_str.is_empty() {
            trimmed.to_string()
        } else {
            prefix_pattern(&prefix_str, trimmed)
        };
        builder
            .add_line(None, &line)
            .with_context(|| format!("parse ignore pattern '{pattern}'"))?;
    }
    builder.build().context("build ignore matcher")
}

fn prefix_pattern(prefix: &str, pattern: &str) -> String {
    let (neg, rest) = match pattern.strip_prefix('!') {
        Some(rest) => ("!", rest),
        None => ("", pattern),
    };
    let rest = rest.trim();
    if rest.starts_with('/') {
        format!("{neg}/{}/{}", prefix, rest.trim_start_matches('/'))
    } else {
        format!("{neg}{}/{}", prefix, rest)
    }
}

fn path_to_slash(path: &Path) -> Result<String> {
    if path.as_os_str().is_empty() {
        return Ok(String::new());
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                let raw = name
                    .to_str()
                    .ok_or_else(|| anyhow!("non-UTF-8 path segment"))?;
                parts.push(raw);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("invalid workspace path prefix");
            }
        }
    }
    Ok(parts.join("/"))
}

fn resolve_workspace_for_sync(
    host: &mut WorldHost<FsStore>,
    workspace: &str,
) -> Result<(String, Option<u64>, bool)> {
    let resolved = workspace_resolve(
        host,
        &WorkspaceResolveParams {
            workspace: workspace.to_string(),
            version: None,
        },
    )?;
    if resolved.exists {
        let root_hash = resolved
            .root_hash
            .clone()
            .ok_or_else(|| anyhow!("workspace root hash missing"))?;
        return Ok((root_hash, resolved.resolved_version, true));
    }
    let receipt = workspace_empty_root(
        host,
        &WorkspaceEmptyRootParams {
            workspace: workspace.to_string(),
        },
    )?;
    Ok((receipt.root_hash, None, false))
}

fn workspace_resolve(
    host: &mut WorldHost<FsStore>,
    params: &WorkspaceResolveParams,
) -> Result<WorkspaceResolveReceipt> {
    handle_internal(
        host,
        EffectKind::workspace_resolve(),
        params,
        "workspace.resolve",
    )
}

fn workspace_empty_root(
    host: &mut WorldHost<FsStore>,
    params: &WorkspaceEmptyRootParams,
) -> Result<WorkspaceEmptyRootReceipt> {
    handle_internal(
        host,
        EffectKind::workspace_empty_root(),
        params,
        "workspace.empty_root",
    )
}

fn workspace_list(
    host: &mut WorldHost<FsStore>,
    params: &WorkspaceListParams,
) -> Result<WorkspaceListReceipt> {
    handle_internal(host, EffectKind::workspace_list(), params, "workspace.list")
}

fn workspace_read_bytes(
    host: &mut WorldHost<FsStore>,
    params: &WorkspaceReadBytesParams,
) -> Result<Vec<u8>> {
    handle_internal(
        host,
        EffectKind::workspace_read_bytes(),
        params,
        "workspace.read_bytes",
    )
}

fn workspace_write_bytes(
    host: &mut WorldHost<FsStore>,
    params: &WorkspaceWriteBytesParams,
) -> Result<WorkspaceWriteBytesReceipt> {
    handle_internal(
        host,
        EffectKind::workspace_write_bytes(),
        params,
        "workspace.write_bytes",
    )
}

fn workspace_remove(
    host: &mut WorldHost<FsStore>,
    params: &WorkspaceRemoveParams,
) -> Result<WorkspaceRemoveReceipt> {
    handle_internal(
        host,
        EffectKind::workspace_remove(),
        params,
        "workspace.remove",
    )
}

fn workspace_annotations_set(
    host: &mut WorldHost<FsStore>,
    params: &WorkspaceAnnotationsSetParams,
) -> Result<WorkspaceAnnotationsSetReceipt> {
    handle_internal(
        host,
        EffectKind::workspace_annotations_set(),
        params,
        "workspace.annotations_set",
    )
}

fn handle_internal<T: serde::de::DeserializeOwned, P: Serialize>(
    host: &mut WorldHost<FsStore>,
    kind: EffectKind,
    params: &P,
    label: &str,
) -> Result<T> {
    let intent = IntentBuilder::new(kind, WORKSPACE_CAP, params)
        .build()
        .map_err(|e| anyhow!("encode {label} params: {e}"))?;
    let receipt = host
        .kernel_mut()
        .handle_internal_intent(&intent)?
        .ok_or_else(|| anyhow!("{label} not handled as internal effect"))?;
    if receipt.status != ReceiptStatus::Ok {
        let err_msg = serde_cbor::from_slice::<String>(&receipt.payload_cbor).ok();
        if let Some(message) = err_msg {
            anyhow::bail!("{label} failed: {message}");
        }
        anyhow::bail!("{label} failed");
    }
    receipt
        .payload::<T>()
        .map_err(|e| anyhow!("decode {label} receipt: {e}"))
}

fn commit_workspace(
    host: &mut WorldHost<FsStore>,
    workspace: &str,
    expected_head: Option<u64>,
    root_hash: &str,
    owner: &str,
) -> Result<()> {
    let payload = build_workspace_commit(workspace, expected_head, root_hash, owner)?;
    host.kernel_mut()
        .submit_domain_event_result(WORKSPACE_EVENT, payload)
        .map_err(|e| anyhow!("workspace commit failed: {e}"))
}

fn build_workspace_commit(
    workspace: &str,
    expected_head: Option<u64>,
    root_hash: &str,
    owner: &str,
) -> Result<Vec<u8>> {
    let created_at = now_ns();
    let event = WorkspaceCommit {
        workspace: workspace.to_string(),
        expected_head,
        meta: WorkspaceCommitMeta {
            root_hash: root_hash.to_string(),
            owner: owner.to_string(),
            created_at,
        },
    };
    serde_cbor::to_vec(&event).context("encode workspace commit")
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
    if let Ok(env_owner) = std::env::var("AOS_OWNER") {
        let trimmed = env_owner.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    for key in ["USER", "LOGNAME", "USERNAME"] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "unknown".into()
}

fn normalize_annotation_key(input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("invalid annotation key");
    }
    Ok(trimmed.to_string())
}

fn file_mode_from_metadata(meta: &fs::Metadata) -> Result<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        if mode & 0o111 != 0 {
            return Ok(MODE_FILE_EXEC);
        }
    }
    Ok(MODE_FILE_DEFAULT)
}

fn set_file_mode(path: &Path, mode: u64) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(mode as u32);
        fs::set_permissions(path, perms)
            .with_context(|| format!("set permissions {}", path.display()))?;
    }
    let _ = mode;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn encode_segment_keeps_safe_tokens() {
        assert_eq!(encode_segment("abcXYZ09._-~"), "abcXYZ09._-~");
        assert_eq!(encode_segment("a~b"), "a~b");
    }

    #[test]
    fn encode_segment_escapes_leading_tilde() {
        assert_eq!(encode_segment("~tilde"), "~7E74696C6465");
    }

    #[test]
    fn decode_segment_round_trips_encoded() {
        assert_eq!(decode_segment("~7E74696C6465").unwrap(), "~tilde");
    }

    #[test]
    fn encode_decode_relative_path_round_trip() {
        let encoded = encode_relative_path(Path::new("foo/bar baz")).unwrap();
        assert_eq!(encoded, "foo/~6261722062617A");
        let decoded = decode_relative_path(&encoded).unwrap();
        assert_eq!(decoded, PathBuf::from("foo/bar baz"));
    }

    #[test]
    fn decode_relative_path_rejects_dot_segments() {
        assert!(decode_relative_path("~2E").is_err());
        assert!(decode_relative_path("~2E2E").is_err());
    }

    #[test]
    fn decode_segment_rejects_invalid_hex() {
        assert!(decode_segment("~").is_err());
        assert!(decode_segment("~0").is_err());
        assert!(decode_segment("~GG").is_err());
    }

    #[test]
    fn decode_segment_rejects_invalid_utf8() {
        assert!(decode_segment("~FF").is_err());
    }

    #[test]
    fn encode_relative_path_rejects_empty() {
        assert!(encode_relative_path(Path::new("")).is_err());
        assert!(encode_relative_path(Path::new(".")).is_err());
    }

    #[test]
    fn ignore_matcher_ignores_unrelated_gitignore_files() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path();
        fs::create_dir_all(root.join(".git")).expect("git dir");
        fs::create_dir_all(root.join(".venv-cbor")).expect("venv dir");
        fs::write(root.join(".venv-cbor/.gitignore"), "*\n").expect("venv ignore");
        let scope = root.join("apps/demiurge/tools");
        fs::create_dir_all(&scope).expect("scope dir");
        fs::write(scope.join("tool.json"), "{}").expect("tool file");

        let matcher = IgnoreMatcher::new(&scope, &[]).expect("matcher");
        assert!(!matcher.is_ignored(Path::new("tool.json"), false));
    }
}
