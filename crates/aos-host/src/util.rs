//! Shared utilities for working with manifests and world directories.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use aos_air_types::{DefModule, HashRef};
use aos_kernel::LoadedManifest;

use crate::manifest_loader::ZERO_HASH_SENTINEL;

/// Check if a module has a placeholder hash that should be patched.
pub fn is_placeholder_hash(module: &DefModule) -> bool {
    module.wasm_hash.as_str() == ZERO_HASH_SENTINEL
}

/// Patch modules in a loaded manifest based on a predicate.
///
/// For each module where `predicate(name, module)` returns true, the module's
/// `wasm_hash` is replaced with the provided hash. Returns the count of modules patched.
///
/// # Examples
///
/// Patch all modules with placeholder hashes:
/// ```ignore
/// let count = patch_modules(&mut loaded, &hash, |_, m| is_placeholder_hash(m));
/// ```
///
/// Patch a specific module by name:
/// ```ignore
/// let count = patch_modules(&mut loaded, &hash, |name, _| name == "demo/MySM@1");
/// ```
pub fn patch_modules(
    loaded: &mut LoadedManifest,
    wasm_hash: &HashRef,
    predicate: impl Fn(&str, &DefModule) -> bool,
) -> usize {
    let mut count = 0;
    for (name, module) in loaded.modules.iter_mut() {
        if predicate(name.as_str(), module) {
            module.wasm_hash = wasm_hash.clone();
            count += 1;
        }
    }
    count
}

/// Check if any modules in the manifest have placeholder hashes.
pub fn has_placeholder_modules(loaded: &LoadedManifest) -> bool {
    loaded.modules.values().any(is_placeholder_hash)
}

/// Remove the journal directory if it exists.
///
/// The journal is located at `<world_root>/.aos/journal/`.
pub fn reset_journal(world_root: &Path) -> Result<()> {
    let journal_dir = world_root.join(".aos/journal");
    if journal_dir.exists() {
        fs::remove_dir_all(&journal_dir).context("remove journal directory")?;
    }
    Ok(())
}
