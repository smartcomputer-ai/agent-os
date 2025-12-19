//! Input parsing utilities for @file and @- syntax.

use std::io::Read;

use anyhow::{Context, Result};

/// Parse an input value that may be a JSON literal, @file, or @- for stdin.
///
/// - `@-` reads from stdin
/// - `@path` reads from the specified file
/// - Otherwise, returns the value as-is (assumed to be JSON literal)
pub fn parse_input_value(value: &str) -> Result<String> {
    if value == "@-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("failed to read from stdin")?;
        Ok(buf)
    } else if let Some(path) = value.strip_prefix('@') {
        std::fs::read_to_string(path).with_context(|| format!("failed to read file: {}", path))
    } else {
        Ok(value.to_string())
    }
}

/// Parse input as raw bytes.
///
/// - `@-` reads from stdin
/// - `@path` reads from the specified file
/// - Otherwise, returns an error (literal data not supported for binary)
#[allow(dead_code)]
pub fn parse_input_bytes(value: &str) -> Result<Vec<u8>> {
    if value == "@-" {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .context("failed to read from stdin")?;
        Ok(buf)
    } else if let Some(path) = value.strip_prefix('@') {
        std::fs::read(path).with_context(|| format!("failed to read file: {}", path))
    } else {
        anyhow::bail!("expected @file or @- for binary input, not literal data")
    }
}
