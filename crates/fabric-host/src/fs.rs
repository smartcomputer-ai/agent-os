//! Confined filesystem operations for fabric session workspaces.

use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
};

use globset::Glob;
use regex::RegexBuilder;
use walkdir::WalkDir;

use crate::{
    patch::{
        EditMatchError, ParsedPatch, PatchOperation, apply_edit, apply_update_hunks,
        parse_patch_v4a,
    },
    runtime::FabricHostError,
    state::HostPaths,
};
use fabric_protocol::{
    FabricBytes, FsApplyPatchRequest, FsApplyPatchResponse, FsDirEntry, FsEditFileRequest,
    FsEditFileResponse, FsEntryKind, FsExistsResponse, FsFileReadResponse, FsFileWriteRequest,
    FsGlobRequest, FsGlobResponse, FsGrepMatch, FsGrepRequest, FsGrepResponse, FsListDirResponse,
    FsMkdirRequest, FsPatchOpsSummary, FsPathQuery, FsRemoveRequest, FsRemoveResponse,
    FsStatResponse, FsWriteResponse, SessionId,
};

const DEFAULT_MAX_READ_BYTES: u64 = 1024 * 1024;
const DEFAULT_MAX_SEARCH_RESULTS: u64 = 1000;

#[derive(Debug, Clone)]
pub struct WorkspaceFs {
    paths: HostPaths,
}

impl WorkspaceFs {
    pub fn new(paths: HostPaths) -> Self {
        Self { paths }
    }

    pub fn read_file(
        &self,
        session_id: &SessionId,
        query: FsPathQuery,
    ) -> Result<FsFileReadResponse, FabricHostError> {
        self.require_session(session_id)?;
        let workspace = self.workspace_root(session_id)?;
        let target = resolve_existing_path(&workspace, &query.path)?;
        let metadata = fs::metadata(&target).map_err(map_io("stat file", &target))?;
        if !metadata.is_file() {
            return Err(FabricHostError::BadRequest(format!(
                "path '{}' is not a file",
                query.path
            )));
        }

        let offset = query.offset_bytes.unwrap_or_default();
        let max_bytes = query.max_bytes.unwrap_or(DEFAULT_MAX_READ_BYTES);
        let mut file = fs::File::open(&target).map_err(map_io("open file", &target))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(map_io("seek file", &target))?;

        let mut limited = file.take(max_bytes);
        let mut data = Vec::new();
        limited
            .read_to_end(&mut data)
            .map_err(map_io("read file", &target))?;

        let bytes_read = data.len() as u64;
        let size_bytes = metadata.len();
        Ok(FsFileReadResponse {
            path: normalize_response_path(&workspace, &target),
            content: FabricBytes::from_bytes_auto(data),
            offset_bytes: offset,
            bytes_read,
            size_bytes,
            truncated: offset.saturating_add(bytes_read) < size_bytes,
            mtime_ns: file_mtime_ns(&target).ok().map(u128::from),
        })
    }

    pub fn write_file(
        &self,
        session_id: &SessionId,
        request: FsFileWriteRequest,
    ) -> Result<FsWriteResponse, FabricHostError> {
        self.require_session(session_id)?;
        let workspace = self.workspace_root(session_id)?;
        let target = resolve_write_path(&workspace, &request.path, request.create_parents)?;
        let content = request
            .content
            .decode_bytes()
            .map_err(FabricHostError::BadRequest)?;
        let mut file = fs::File::create(&target).map_err(map_io("create file", &target))?;
        file.write_all(&content)
            .map_err(map_io("write file", &target))?;

        Ok(FsWriteResponse {
            path: normalize_response_path(&workspace, &target),
            bytes_written: content.len() as u64,
        })
    }

    pub fn edit_file(
        &self,
        session_id: &SessionId,
        request: FsEditFileRequest,
    ) -> Result<FsEditFileResponse, FabricHostError> {
        self.require_session(session_id)?;
        if request.old_string.is_empty() {
            return Err(FabricHostError::BadRequest(
                "old_string must not be empty".to_owned(),
            ));
        }

        let workspace = self.workspace_root(session_id)?;
        let target = resolve_existing_path(&workspace, &request.path)?;
        let content = fs::read_to_string(&target).map_err(map_io("read file", &target))?;
        let result = apply_edit(
            &content,
            &request.old_string,
            &request.new_string,
            request.replace_all,
        )
        .map_err(|error| edit_error(&request.path, error))?;

        write_file_atomic(&target, result.updated.as_bytes())?;
        Ok(FsEditFileResponse {
            path: normalize_response_path(&workspace, &target),
            replacements: result.replacements as u64,
            applied: true,
        })
    }

    pub fn apply_patch(
        &self,
        session_id: &SessionId,
        request: FsApplyPatchRequest,
    ) -> Result<FsApplyPatchResponse, FabricHostError> {
        self.require_session(session_id)?;
        let patch_format = request.patch_format.as_deref().unwrap_or("v4a");
        if patch_format != "v4a" {
            return Err(FabricHostError::BadRequest(format!(
                "unsupported patch format '{patch_format}'"
            )));
        }
        if request.patch.trim().is_empty() {
            return Err(FabricHostError::BadRequest(
                "patch must not be empty".to_owned(),
            ));
        }

        let parsed = parse_patch_v4a(&request.patch)
            .map_err(|error| FabricHostError::BadRequest(format!("patch parse error: {error}")))?;
        let workspace = self.workspace_root(session_id)?;
        apply_patch_to_workspace(&workspace, &parsed, request.dry_run)
    }

    pub fn mkdir(
        &self,
        session_id: &SessionId,
        request: FsMkdirRequest,
    ) -> Result<FsStatResponse, FabricHostError> {
        self.require_session(session_id)?;
        let workspace = self.workspace_root(session_id)?;
        let target = resolve_create_path(&workspace, &request.path)?;

        if request.parents {
            secure_create_dir_all(&workspace, &target)?;
        } else {
            let parent = target.parent().ok_or_else(|| {
                FabricHostError::BadRequest(format!("path '{}' has no parent", request.path))
            })?;
            ensure_inside_workspace(&workspace, &canonicalize_existing(parent)?)?;
            fs::create_dir(&target).map_err(map_io("create directory", &target))?;
        }

        stat_path(&workspace, &target, &request.path)
    }

    pub fn remove(
        &self,
        session_id: &SessionId,
        request: FsRemoveRequest,
    ) -> Result<FsRemoveResponse, FabricHostError> {
        self.require_session(session_id)?;
        let workspace = self.workspace_root(session_id)?;
        let target = resolve_existing_path(&workspace, &request.path)?;
        let metadata = fs::symlink_metadata(&target).map_err(map_io("stat path", &target))?;

        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            if request.recursive {
                fs::remove_dir_all(&target).map_err(map_io("remove directory", &target))?;
            } else {
                fs::remove_dir(&target).map_err(map_io("remove directory", &target))?;
            }
        } else {
            fs::remove_file(&target).map_err(map_io("remove file", &target))?;
        }

        Ok(FsRemoveResponse {
            path: request.path,
            removed: true,
        })
    }

    pub fn exists(
        &self,
        session_id: &SessionId,
        query: FsPathQuery,
    ) -> Result<FsExistsResponse, FabricHostError> {
        self.require_session(session_id)?;
        let workspace = self.workspace_root(session_id)?;
        let relative = workspace_relative_path(&query.path)?;
        let target = workspace.join(relative);
        let exists = if target.exists() {
            resolve_existing_path(&workspace, &query.path).is_ok()
        } else {
            false
        };
        Ok(FsExistsResponse {
            path: normalize_response_path(&workspace, &target),
            exists,
        })
    }

    pub fn stat(
        &self,
        session_id: &SessionId,
        query: FsPathQuery,
    ) -> Result<FsStatResponse, FabricHostError> {
        self.require_session(session_id)?;
        let workspace = self.workspace_root(session_id)?;
        let target = resolve_existing_path(&workspace, &query.path)?;
        stat_path(&workspace, &target, &query.path)
    }

    pub fn list_dir(
        &self,
        session_id: &SessionId,
        query: FsPathQuery,
    ) -> Result<FsListDirResponse, FabricHostError> {
        self.require_session(session_id)?;
        let workspace = self.workspace_root(session_id)?;
        let target = resolve_existing_path(&workspace, &query.path)?;
        let metadata = fs::metadata(&target).map_err(map_io("stat directory", &target))?;
        if !metadata.is_dir() {
            return Err(FabricHostError::BadRequest(format!(
                "path '{}' is not a directory",
                query.path
            )));
        }

        let mut entries = Vec::new();
        for entry in fs::read_dir(&target).map_err(map_io("list directory", &target))? {
            let entry = entry.map_err(|error| {
                FabricHostError::Runtime(format!(
                    "read directory entry '{}': {error}",
                    target.display()
                ))
            })?;
            let path = entry.path();
            let resolved =
                resolve_existing_path(&workspace, &normalize_response_path(&workspace, &path))?;
            let metadata =
                fs::symlink_metadata(&resolved).map_err(map_io("stat entry", &resolved))?;
            entries.push(FsDirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: normalize_response_path(&workspace, &resolved),
                kind: entry_kind(&metadata),
                size_bytes: metadata.len(),
                readonly: metadata.permissions().readonly(),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(FsListDirResponse {
            path: normalize_response_path(&workspace, &target),
            entries,
        })
    }

    pub fn grep(
        &self,
        session_id: &SessionId,
        request: FsGrepRequest,
    ) -> Result<FsGrepResponse, FabricHostError> {
        self.require_session(session_id)?;
        if request.pattern.is_empty() {
            return Err(FabricHostError::BadRequest(
                "grep pattern must not be empty".to_owned(),
            ));
        }

        let workspace = self.workspace_root(session_id)?;
        let base = resolve_existing_path(&workspace, request.path.as_deref().unwrap_or("."))?;
        let regex = RegexBuilder::new(&request.pattern)
            .case_insensitive(request.case_insensitive)
            .build()
            .map_err(|error| FabricHostError::BadRequest(format!("invalid regex: {error}")))?;
        let matcher = if let Some(filter) = request.glob_filter.as_deref() {
            Some(
                Glob::new(filter)
                    .map_err(|error| {
                        FabricHostError::BadRequest(format!("invalid glob filter: {error}"))
                    })?
                    .compile_matcher(),
            )
        } else {
            None
        };
        let max_results = request
            .max_results
            .unwrap_or(DEFAULT_MAX_SEARCH_RESULTS)
            .max(1);

        let mut matches = Vec::new();
        let mut truncated = false;
        for file in collect_files(&base)? {
            let relative_to_base = display_relative(&base, &file);
            if let Some(matcher) = &matcher {
                if !matcher.is_match(Path::new(&relative_to_base)) {
                    continue;
                }
            }

            let content = fs::read(&file).map_err(map_io("read file", &file))?;
            let content = String::from_utf8_lossy(&content);
            for (line_index, line) in content.lines().enumerate() {
                if !regex.is_match(line) {
                    continue;
                }
                if matches.len() as u64 >= max_results {
                    truncated = true;
                    break;
                }
                matches.push(FsGrepMatch {
                    path: normalize_response_path(&workspace, &file),
                    line_number: line_index as u64 + 1,
                    line: line.to_owned(),
                });
            }
            if truncated {
                break;
            }
        }

        Ok(FsGrepResponse {
            match_count: matches.len() as u64,
            matches,
            truncated,
        })
    }

    pub fn glob(
        &self,
        session_id: &SessionId,
        request: FsGlobRequest,
    ) -> Result<FsGlobResponse, FabricHostError> {
        self.require_session(session_id)?;
        if request.pattern.is_empty() {
            return Err(FabricHostError::BadRequest(
                "glob pattern must not be empty".to_owned(),
            ));
        }

        let workspace = self.workspace_root(session_id)?;
        let base = resolve_existing_path(&workspace, request.path.as_deref().unwrap_or("."))?;
        let matcher = Glob::new(&request.pattern)
            .map_err(|error| FabricHostError::BadRequest(format!("invalid glob: {error}")))?
            .compile_matcher();
        let max_results = request
            .max_results
            .unwrap_or(DEFAULT_MAX_SEARCH_RESULTS)
            .max(1);

        let mut entries = Vec::new();
        for file in collect_files(&base)? {
            let relative_to_base = display_relative(&base, &file);
            if matcher.is_match(Path::new(&relative_to_base)) {
                entries.push((
                    normalize_response_path(&workspace, &file),
                    file_mtime_ns(&file)?,
                ));
            }
        }

        entries.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
        let truncated = entries.len() as u64 > max_results;
        let paths = entries
            .into_iter()
            .take(max_results as usize)
            .map(|(path, _)| path)
            .collect::<Vec<_>>();

        Ok(FsGlobResponse {
            count: paths.len() as u64,
            paths,
            truncated,
        })
    }

    fn require_session(&self, session_id: &SessionId) -> Result<(), FabricHostError> {
        self.paths
            .read_marker(session_id)?
            .ok_or_else(|| {
                FabricHostError::NotFound(format!("session workspace '{}'", session_id.0))
            })
            .map(|_| ())
    }

    fn workspace_root(&self, session_id: &SessionId) -> Result<PathBuf, FabricHostError> {
        let workspace = self.paths.workspace(session_id);
        fs::create_dir_all(&workspace).map_err(map_io("create workspace", &workspace))?;
        canonicalize_existing(&workspace)
    }
}

fn resolve_existing_path(workspace: &Path, request_path: &str) -> Result<PathBuf, FabricHostError> {
    let target = workspace.join(workspace_relative_path(request_path)?);
    let resolved = canonicalize_existing(&target)?;
    ensure_inside_workspace(workspace, &resolved)?;
    Ok(target)
}

fn resolve_write_path(
    workspace: &Path,
    request_path: &str,
    create_parents: bool,
) -> Result<PathBuf, FabricHostError> {
    let target = workspace.join(workspace_relative_path(request_path)?);
    if let Ok(existing) = canonicalize_existing(&target) {
        ensure_inside_workspace(workspace, &existing)?;
        let metadata = fs::metadata(&existing).map_err(map_io("stat file", &existing))?;
        if metadata.is_dir() {
            return Err(FabricHostError::BadRequest(format!(
                "path '{request_path}' is a directory"
            )));
        }
        return Ok(target);
    }

    let parent = target.parent().ok_or_else(|| {
        FabricHostError::BadRequest(format!("path '{request_path}' has no parent"))
    })?;
    if create_parents {
        secure_create_dir_all(workspace, parent)?;
    }
    let parent = canonicalize_existing(parent)?;
    ensure_inside_workspace(workspace, &parent)?;
    Ok(target)
}

fn resolve_create_path(workspace: &Path, request_path: &str) -> Result<PathBuf, FabricHostError> {
    let target = workspace.join(workspace_relative_path(request_path)?);
    if let Ok(existing) = canonicalize_existing(&target) {
        ensure_inside_workspace(workspace, &existing)?;
        return Ok(target);
    }
    let parent = target.parent().ok_or_else(|| {
        FabricHostError::BadRequest(format!("path '{request_path}' has no parent"))
    })?;
    if parent.exists() {
        ensure_inside_workspace(workspace, &canonicalize_existing(parent)?)?;
    }
    Ok(target)
}

fn secure_create_dir_all(workspace: &Path, target: &Path) -> Result<(), FabricHostError> {
    let relative = target.strip_prefix(workspace).map_err(|_| {
        FabricHostError::BadRequest(format!(
            "path '{}' escapes workspace '{}'",
            target.display(),
            workspace.display()
        ))
    })?;

    let mut current = workspace.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(part) => current.push(part),
            Component::CurDir => continue,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(FabricHostError::BadRequest(format!(
                    "path '{}' is not workspace-relative",
                    target.display()
                )));
            }
        }

        if current.exists() {
            let resolved = canonicalize_existing(&current)?;
            ensure_inside_workspace(workspace, &resolved)?;
            let metadata = fs::metadata(&current).map_err(map_io("stat directory", &current))?;
            if !metadata.is_dir() {
                return Err(FabricHostError::BadRequest(format!(
                    "path '{}' is not a directory",
                    current.display()
                )));
            }
        } else {
            fs::create_dir(&current).map_err(map_io("create directory", &current))?;
        }
    }
    Ok(())
}

fn workspace_relative_path(request_path: &str) -> Result<PathBuf, FabricHostError> {
    let trimmed = request_path.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == "/workspace" {
        return Ok(PathBuf::new());
    }

    let path = if let Some(rest) = trimmed.strip_prefix("/workspace/") {
        Path::new(rest)
    } else if trimmed.starts_with('/') {
        return Err(FabricHostError::BadRequest(format!(
            "absolute path '{trimmed}' must be under /workspace"
        )));
    } else {
        Path::new(trimmed)
    };

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(FabricHostError::BadRequest(format!(
                    "path '{trimmed}' must not contain '..'"
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(FabricHostError::BadRequest(format!(
                    "path '{trimmed}' is not workspace-relative"
                )));
            }
        }
    }
    Ok(normalized)
}

fn canonicalize_existing(path: &Path) -> Result<PathBuf, FabricHostError> {
    path.canonicalize()
        .map_err(map_io("canonicalize path", path))
}

fn ensure_inside_workspace(workspace: &Path, path: &Path) -> Result<(), FabricHostError> {
    if path.starts_with(workspace) {
        Ok(())
    } else {
        Err(FabricHostError::BadRequest(format!(
            "path '{}' escapes workspace '{}'",
            path.display(),
            workspace.display()
        )))
    }
}

fn stat_path(
    workspace: &Path,
    path: &Path,
    _request_path: &str,
) -> Result<FsStatResponse, FabricHostError> {
    let metadata = fs::symlink_metadata(path).map_err(map_io("stat path", path))?;
    Ok(FsStatResponse {
        path: normalize_response_path(workspace, path),
        kind: entry_kind(&metadata),
        size_bytes: metadata.len(),
        readonly: metadata.permissions().readonly(),
        mtime_ns: file_mtime_ns(path).ok().map(u128::from),
    })
}

fn entry_kind(metadata: &fs::Metadata) -> FsEntryKind {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        FsEntryKind::Symlink
    } else if file_type.is_dir() {
        FsEntryKind::Directory
    } else if file_type.is_file() {
        FsEntryKind::File
    } else {
        FsEntryKind::Other
    }
}

fn normalize_response_path(workspace: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(workspace).unwrap_or(path);
    if relative.as_os_str().is_empty() {
        "/workspace".to_owned()
    } else {
        format!("/workspace/{}", relative.to_string_lossy())
    }
}

fn apply_patch_to_workspace(
    workspace: &Path,
    parsed: &ParsedPatch,
    dry_run: bool,
) -> Result<FsApplyPatchResponse, FabricHostError> {
    let mut staged: BTreeMap<PathBuf, Option<Vec<u8>>> = BTreeMap::new();
    let mut original: BTreeMap<PathBuf, Option<Vec<u8>>> = BTreeMap::new();

    for op in &parsed.operations {
        match op {
            PatchOperation::AddFile { path, lines } => {
                let resolved = resolve_create_path(workspace, path)?;
                let current = read_staged_or_disk(&resolved, &staged, &mut original)?;
                if current.is_some() {
                    return Err(FabricHostError::BadRequest(format!(
                        "file '{path}' already exists"
                    )));
                }
                staged.insert(resolved, Some(lines.join("\n").into_bytes()));
            }
            PatchOperation::DeleteFile { path } => {
                let resolved = resolve_existing_path(workspace, path)?;
                let current = read_staged_or_disk(&resolved, &staged, &mut original)?;
                if current.is_none() {
                    return Err(FabricHostError::NotFound(format!("file '{path}'")));
                }
                staged.insert(resolved, None);
            }
            PatchOperation::UpdateFile {
                path,
                move_to,
                hunks,
            } => {
                let source = resolve_existing_path(workspace, path)?;
                let current = read_staged_or_disk(&source, &staged, &mut original)?;
                let Some(current_bytes) = current else {
                    return Err(FabricHostError::NotFound(format!("file '{path}'")));
                };
                let current_text = String::from_utf8(current_bytes).map_err(|_| {
                    FabricHostError::BadRequest(format!("file '{path}' is not utf8"))
                })?;
                let updated_text = apply_update_hunks(&current_text, hunks)
                    .map_err(|error| FabricHostError::BadRequest(format!("{path}: {error}")))?;

                let target = if let Some(move_to) = move_to {
                    resolve_create_path(workspace, move_to)?
                } else {
                    source.clone()
                };

                if target != source {
                    let current_target = read_staged_or_disk(&target, &staged, &mut original)?;
                    if current_target.is_some() {
                        return Err(FabricHostError::BadRequest(format!(
                            "move target '{}' already exists",
                            move_to.clone().unwrap_or_default()
                        )));
                    }
                    staged.insert(source, None);
                }

                staged.insert(target, Some(updated_text.into_bytes()));
            }
        }
    }

    let mut changed_paths = Vec::new();
    for (path, next) in &staged {
        let prev = original.get(path).cloned().unwrap_or(None);
        if prev != *next {
            changed_paths.push(normalize_response_path(workspace, path));
        }
    }
    changed_paths.sort();

    if !dry_run {
        for (path, maybe_bytes) in &staged {
            match maybe_bytes {
                Some(bytes) => {
                    if let Some(parent) = path.parent() {
                        secure_create_dir_all(workspace, parent)?;
                    }
                    write_file_atomic(path, bytes)?;
                }
                None => {
                    if path.exists() {
                        fs::remove_file(path).map_err(map_io("delete file", path))?;
                    }
                }
            }
        }
    }

    Ok(FsApplyPatchResponse {
        files_changed: changed_paths.len() as u64,
        changed_paths,
        ops: FsPatchOpsSummary {
            add: parsed.counts.add,
            update: parsed.counts.update,
            delete: parsed.counts.delete,
            move_count: parsed.counts.move_count,
        },
        applied: !dry_run,
    })
}

fn read_staged_or_disk(
    path: &Path,
    staged: &BTreeMap<PathBuf, Option<Vec<u8>>>,
    original: &mut BTreeMap<PathBuf, Option<Vec<u8>>>,
) -> Result<Option<Vec<u8>>, FabricHostError> {
    if let Some(value) = staged.get(path) {
        return Ok(value.clone());
    }
    if let Some(value) = original.get(path) {
        return Ok(value.clone());
    }
    match fs::read(path) {
        Ok(bytes) => {
            original.insert(path.to_path_buf(), Some(bytes.clone()));
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            original.insert(path.to_path_buf(), None);
            Ok(None)
        }
        Err(error) => Err(map_io("read file", path)(error)),
    }
}

fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<(), FabricHostError> {
    let parent = path.parent().ok_or_else(|| {
        FabricHostError::BadRequest(format!("path '{}' has no parent", path.display()))
    })?;
    let tmp_path = parent.join(format!(
        ".{}.fabric-tmp-{}",
        path.file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default(),
        uuid::Uuid::new_v4()
    ));
    fs::write(&tmp_path, bytes).map_err(map_io("write temp file", &tmp_path))?;
    fs::rename(&tmp_path, path).map_err(map_io("rename temp file", path))?;
    Ok(())
}

fn collect_files(base: &Path) -> Result<Vec<PathBuf>, FabricHostError> {
    let metadata = fs::metadata(base).map_err(map_io("stat path", base))?;
    if metadata.is_file() {
        return Ok(vec![base.to_path_buf()]);
    }
    if !metadata.is_dir() {
        return Err(FabricHostError::BadRequest(format!(
            "path '{}' is not a file or directory",
            base.display()
        )));
    }

    let mut files = Vec::new();
    for entry in WalkDir::new(base).follow_links(false).into_iter() {
        let entry =
            entry.map_err(|error| FabricHostError::Runtime(format!("walk directory: {error}")))?;
        if entry.file_type().is_file() {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    Ok(files)
}

fn display_relative(base: &Path, path: &Path) -> String {
    if base.is_file() {
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string())
    } else {
        path.strip_prefix(base)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string()
    }
}

fn file_mtime_ns(path: &Path) -> Result<u64, FabricHostError> {
    let metadata = fs::metadata(path).map_err(map_io("stat file", path))?;
    let modified = metadata.modified().map_err(|error| {
        FabricHostError::Runtime(format!("read mtime '{}': {error}", path.display()))
    })?;
    let duration = modified
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    Ok(duration
        .as_secs()
        .saturating_mul(1_000_000_000)
        .saturating_add(u64::from(duration.subsec_nanos())))
}

fn edit_error(path: &str, error: EditMatchError) -> FabricHostError {
    match error {
        EditMatchError::NotFound => {
            FabricHostError::BadRequest(format!("old_string not found in '{path}'"))
        }
        EditMatchError::Ambiguous(count) => FabricHostError::BadRequest(format!(
            "old_string matched {count} times in '{path}'; set replace_all to true"
        )),
    }
}

fn map_io<'a>(
    action: &'static str,
    path: &'a Path,
) -> impl FnOnce(std::io::Error) -> FabricHostError + 'a {
    move |error| FabricHostError::Runtime(format!("{action} '{}': {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::FabricSessionMarker;
    use fabric_protocol::{NetworkMode, SessionId};

    fn test_fs() -> (WorkspaceFs, HostPaths, SessionId, PathBuf) {
        let session_id = SessionId(format!("sess-test-{}", uuid::Uuid::new_v4()));
        let state_root = std::env::temp_dir().join(format!("fabric-fs-{}", uuid::Uuid::new_v4()));
        let paths = HostPaths::new(&state_root);
        paths.ensure_session_dirs(&session_id).unwrap();
        paths
            .write_marker(&FabricSessionMarker {
                host_id: "test-host".to_owned(),
                session_id: session_id.clone(),
                machine_name: "test-machine".to_owned(),
                image: "alpine:latest".to_owned(),
                workspace_path: paths.workspace(&session_id),
                workdir: "/workspace".to_owned(),
                network_mode: NetworkMode::Disabled,
                status: None,
                created_at_ns: 0,
                expires_at_ns: None,
                labels: Default::default(),
            })
            .unwrap();

        (
            WorkspaceFs::new(paths.clone()),
            paths,
            session_id,
            state_root,
        )
    }

    #[test]
    fn write_read_and_list_are_workspace_relative() {
        let (fs, _paths, session_id, state_root) = test_fs();

        fs.write_file(
            &session_id,
            FsFileWriteRequest {
                path: "dir/hello.txt".to_owned(),
                content: FabricBytes::Text("hello".to_owned()),
                create_parents: true,
            },
        )
        .unwrap();

        let read = fs
            .read_file(
                &session_id,
                FsPathQuery {
                    path: "/workspace/dir/hello.txt".to_owned(),
                    offset_bytes: None,
                    max_bytes: None,
                },
            )
            .unwrap();
        assert_eq!(read.content.as_text(), Some("hello"));
        assert!(read.mtime_ns.is_some());

        let binary = vec![0, 159, 255, b'\n'];
        fs.write_file(
            &session_id,
            FsFileWriteRequest {
                path: "dir/blob.bin".to_owned(),
                content: FabricBytes::from_bytes_base64(&binary),
                create_parents: true,
            },
        )
        .unwrap();
        let binary_read = fs
            .read_file(
                &session_id,
                FsPathQuery {
                    path: "dir/blob.bin".to_owned(),
                    offset_bytes: None,
                    max_bytes: None,
                },
            )
            .unwrap();
        assert_eq!(binary_read.content.decode_bytes().unwrap(), binary);
        assert!(matches!(binary_read.content, FabricBytes::Base64(_)));

        let listed = fs
            .list_dir(
                &session_id,
                FsPathQuery {
                    path: "dir".to_owned(),
                    offset_bytes: None,
                    max_bytes: None,
                },
            )
            .unwrap();
        assert!(
            listed
                .entries
                .iter()
                .any(|entry| entry.path == "/workspace/dir/hello.txt")
        );

        let _ = std::fs::remove_dir_all(state_root);
    }

    #[test]
    fn dot_dot_paths_are_rejected() {
        let (fs, _paths, session_id, state_root) = test_fs();
        let result = fs.read_file(
            &session_id,
            FsPathQuery {
                path: "../escape".to_owned(),
                offset_bytes: None,
                max_bytes: None,
            },
        );
        assert!(matches!(result, Err(FabricHostError::BadRequest(_))));
        let _ = std::fs::remove_dir_all(state_root);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        let (fs, paths, session_id, state_root) = test_fs();
        let outside =
            std::env::temp_dir().join(format!("fabric-fs-outside-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(
            outside.join("secret.txt"),
            paths.workspace(&session_id).join("secret-link"),
        )
        .unwrap();

        let result = fs.read_file(
            &session_id,
            FsPathQuery {
                path: "secret-link".to_owned(),
                offset_bytes: None,
                max_bytes: None,
            },
        );
        assert!(matches!(result, Err(FabricHostError::BadRequest(_))));

        let _ = std::fs::remove_dir_all(outside);
        let _ = std::fs::remove_dir_all(state_root);
    }

    #[test]
    fn edit_patch_grep_and_glob_are_workspace_relative() {
        let (fs, _paths, session_id, state_root) = test_fs();

        fs.write_file(
            &session_id,
            FsFileWriteRequest {
                path: "src/main.rs".to_owned(),
                content: FabricBytes::Text(
                    "fn main() {\n    println!(\"hello world\");\n}\n".to_owned(),
                ),
                create_parents: true,
            },
        )
        .unwrap();

        let edited = fs
            .edit_file(
                &session_id,
                FsEditFileRequest {
                    path: "src/main.rs".to_owned(),
                    old_string: "hello world".to_owned(),
                    new_string: "hello fabric".to_owned(),
                    replace_all: false,
                },
            )
            .unwrap();
        assert_eq!(edited.replacements, 1);

        let patch = "\
*** Begin Patch
*** Add File: src/lib.rs
+pub fn name() -> &'static str {
+    \"fabric\"
+}
*** Update File: src/main.rs
@@
-    println!(\"hello fabric\");
+    println!(\"hello from patch\");
*** End Patch";
        let patched = fs
            .apply_patch(
                &session_id,
                FsApplyPatchRequest {
                    patch: patch.to_owned(),
                    patch_format: Some("v4a".to_owned()),
                    dry_run: false,
                },
            )
            .unwrap();
        assert_eq!(patched.files_changed, 2);
        assert!(
            patched
                .changed_paths
                .contains(&"/workspace/src/lib.rs".to_owned())
        );

        let grep = fs
            .grep(
                &session_id,
                FsGrepRequest {
                    pattern: "fabric|patch".to_owned(),
                    path: Some("src".to_owned()),
                    glob_filter: Some("*.rs".to_owned()),
                    max_results: Some(10),
                    case_insensitive: false,
                },
            )
            .unwrap();
        assert!(grep.match_count >= 2);
        assert!(
            grep.matches
                .iter()
                .any(|matched| matched.path == "/workspace/src/main.rs")
        );

        let glob = fs
            .glob(
                &session_id,
                FsGlobRequest {
                    pattern: "*.rs".to_owned(),
                    path: Some("src".to_owned()),
                    max_results: Some(10),
                },
            )
            .unwrap();
        assert_eq!(glob.count, 2);
        assert!(glob.paths.contains(&"/workspace/src/main.rs".to_owned()));
        assert!(glob.paths.contains(&"/workspace/src/lib.rs".to_owned()));

        let _ = std::fs::remove_dir_all(state_root);
    }
}
