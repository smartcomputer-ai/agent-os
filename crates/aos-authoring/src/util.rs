use aos_air_types::{DefModule, HashRef};
use aos_kernel::LoadedManifest;

use crate::manifest_loader::ZERO_HASH_SENTINEL;

pub fn is_placeholder_hash(module: &DefModule) -> bool {
    module.wasm_hash.as_str() == ZERO_HASH_SENTINEL
}

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

pub fn has_placeholder_modules(loaded: &LoadedManifest) -> bool {
    loaded.modules.values().any(is_placeholder_hash)
}
