//! File and URL helpers for multimodal inputs.

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use mime_guess::MimeGuess;

/// Detect whether a string looks like a local file path.
pub fn is_local_path(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("./")
        || value.starts_with("../")
        || value.starts_with('~')
}

/// Expand a path with a leading ~ to the user's home directory.
pub fn expand_tilde(value: &str) -> PathBuf {
    if let Some(stripped) = value.strip_prefix('~') {
        if let Some(home) = std::env::var_os("HOME") {
            let mut base = PathBuf::from(home);
            let trimmed = stripped.trim_start_matches('/');
            base.push(trimmed);
            return base;
        }
    }
    PathBuf::from(value)
}

/// Infer the MIME type from a file path.
pub fn infer_mime_type(path: &Path) -> Option<String> {
    MimeGuess::from_path(path)
        .first()
        .map(|mime| mime.essence_str().to_string())
}

/// Load a local file into memory with base64 encoding and MIME type inference.
pub fn load_file_data(path: &Path) -> std::io::Result<FileData> {
    let bytes = fs::read(path)?;
    let base64 = BASE64.encode(&bytes);
    let media_type = infer_mime_type(path);
    Ok(FileData {
        bytes,
        base64,
        media_type,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileData {
    pub bytes: Vec<u8>,
    pub base64: String,
    pub media_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_local_paths() {
        assert!(is_local_path("/tmp/file.png"));
        assert!(is_local_path("./file.png"));
        assert!(is_local_path("../file.png"));
        assert!(is_local_path("~/file.png"));
        assert!(!is_local_path("https://example.com/file.png"));
    }
}
