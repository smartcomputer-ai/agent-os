use std::path::{Component, Path, PathBuf};

use super::state::SessionRecord;

#[derive(Debug)]
pub(crate) struct PathResolveError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl PathResolveError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_path",
            message: message.into(),
        }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            code: "forbidden",
            message: message.into(),
        }
    }

    fn io(message: impl Into<String>) -> Self {
        Self {
            code: "io_error",
            message: message.into(),
        }
    }
}

pub(crate) fn resolve_session_path(
    session: &SessionRecord,
    raw_path: &str,
) -> Result<PathBuf, PathResolveError> {
    if raw_path.trim().is_empty() {
        return Err(PathResolveError::invalid("path must not be empty"));
    }

    let path = PathBuf::from(raw_path);
    let absolute = if path.is_absolute() {
        path
    } else {
        session.workdir.join(path)
    };
    let normalized = normalize_lexical(&absolute)?;

    if !normalized.starts_with(&session.workdir) {
        return Err(PathResolveError::forbidden(format!(
            "path '{}' escapes session root",
            raw_path
        )));
    }

    ensure_existing_parent_within_root(&session.workdir, &normalized)?;
    if normalized.exists() {
        ensure_canonical_within_root(&session.workdir, &normalized)?;
    }

    Ok(normalized)
}

pub(crate) fn resolve_session_base(
    session: &SessionRecord,
    raw_path: Option<&str>,
) -> Result<PathBuf, PathResolveError> {
    match raw_path {
        Some(path) => resolve_session_path(session, path),
        None => Ok(session.workdir.clone()),
    }
}

pub(crate) fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

fn normalize_lexical(path: &Path) -> Result<PathBuf, PathResolveError> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return Err(PathResolveError::invalid(format!(
                        "cannot resolve path '{}'",
                        path.to_string_lossy()
                    )));
                }
            }
            Component::Normal(value) => out.push(value),
        }
    }

    if !out.is_absolute() {
        return Err(PathResolveError::invalid(format!(
            "path '{}' must resolve to an absolute path",
            path.to_string_lossy()
        )));
    }
    Ok(out)
}

fn ensure_existing_parent_within_root(root: &Path, path: &Path) -> Result<(), PathResolveError> {
    let mut probe = path.to_path_buf();
    while !probe.exists() {
        let Some(parent) = probe.parent() else {
            return Err(PathResolveError::invalid(format!(
                "path '{}' has no resolvable parent",
                path.to_string_lossy()
            )));
        };
        probe = parent.to_path_buf();
    }

    ensure_canonical_within_root(root, &probe)
}

fn ensure_canonical_within_root(root: &Path, path: &Path) -> Result<(), PathResolveError> {
    let canonical_root = std::fs::canonicalize(root)
        .map_err(|err| PathResolveError::io(format!("canonicalize root: {err}")))?;
    let canonical = std::fs::canonicalize(path)
        .map_err(|err| PathResolveError::io(format!("canonicalize path: {err}")))?;

    if !canonical.starts_with(&canonical_root) {
        return Err(PathResolveError::forbidden(format!(
            "path '{}' escapes session root via symlink traversal",
            path.to_string_lossy()
        )));
    }
    Ok(())
}
