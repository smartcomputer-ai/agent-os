use std::path::{Component, Path, PathBuf};

use anyhow::{Result, anyhow};

#[derive(Debug, Clone)]
pub struct WorkspaceRef {
    pub workspace: String,
    pub version: Option<u64>,
    pub path: Option<String>,
}

pub fn parse_workspace_ref(reference: &str) -> Result<WorkspaceRef> {
    let (head, path) = match reference.split_once('/') {
        Some((head, tail)) => (head, Some(tail.to_string())),
        None => (reference, None),
    };
    let (workspace, version) = match head.rsplit_once('@') {
        Some((workspace, version)) if !workspace.is_empty() && !version.is_empty() => {
            let version = version
                .parse::<u64>()
                .map_err(|_| anyhow!("invalid workspace version in '{reference}'"))?;
            (workspace.to_string(), Some(version))
        }
        _ => (head.to_string(), None),
    };
    if workspace.trim().is_empty() {
        return Err(anyhow!("invalid workspace reference '{reference}'"));
    }
    let path = path.filter(|value| !value.is_empty());
    Ok(WorkspaceRef {
        workspace,
        version,
        path,
    })
}

pub fn encode_relative_path(path: &Path) -> Result<String> {
    let mut out = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => out.push(value.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => {
                return Err(anyhow!(
                    "workspace paths must be relative: {}",
                    path.display()
                ));
            }
        }
    }
    Ok(out.join("/"))
}

pub fn decode_relative_path(path: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for segment in path.split('/') {
        if segment.is_empty() {
            continue;
        }
        out.push(segment);
    }
    out
}

pub fn join_workspace_path(base: Option<&str>, child: &str) -> String {
    match base {
        Some(base) if !base.is_empty() => format!("{base}/{child}"),
        _ => child.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_workspace_ref_with_version_and_path() {
        let parsed = parse_workspace_ref("docs@7/src/lib.rs").expect("parse workspace ref");
        assert_eq!(parsed.workspace, "docs");
        assert_eq!(parsed.version, Some(7));
        assert_eq!(parsed.path.as_deref(), Some("src/lib.rs"));
    }

    #[test]
    fn parse_workspace_ref_without_version() {
        let parsed = parse_workspace_ref("docs/assets").expect("parse workspace ref");
        assert_eq!(parsed.workspace, "docs");
        assert_eq!(parsed.version, None);
        assert_eq!(parsed.path.as_deref(), Some("assets"));
    }

    #[test]
    fn encode_relative_path_rejects_parent_segments() {
        let err = encode_relative_path(Path::new("../secret.txt")).expect_err("reject parent path");
        assert!(err.to_string().contains("workspace paths must be relative"));
    }
}
