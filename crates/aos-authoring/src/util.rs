use aos_air_types::{DefModule, HashRef, ModuleRuntime, WasmArtifact};
use aos_kernel::LoadedManifest;

use crate::manifest_loader::ZERO_HASH_SENTINEL;

pub fn wasm_module_hash(module: &DefModule) -> Option<&HashRef> {
    match &module.runtime {
        ModuleRuntime::Wasm {
            artifact: WasmArtifact::WasmModule { hash },
        } => Some(hash),
        _ => None,
    }
}

pub fn is_placeholder_hash(module: &DefModule) -> bool {
    wasm_module_hash(module)
        .map(|hash| hash.as_str() == ZERO_HASH_SENTINEL)
        .unwrap_or(false)
}

pub fn set_module_wasm_hash(module: &mut DefModule, wasm_hash: HashRef) -> bool {
    match &mut module.runtime {
        ModuleRuntime::Wasm {
            artifact: WasmArtifact::WasmModule { hash },
        } => {
            *hash = wasm_hash;
            true
        }
        _ => false,
    }
}

pub fn patch_modules(
    loaded: &mut LoadedManifest,
    wasm_hash: &HashRef,
    predicate: impl Fn(&str, &DefModule) -> bool,
) -> usize {
    let mut count = 0;
    for (name, module) in loaded.modules.iter_mut() {
        if predicate(name.as_str(), module) {
            if set_module_wasm_hash(module, wasm_hash.clone()) {
                count += 1;
            }
        }
    }
    count
}

pub fn has_placeholder_modules(loaded: &LoadedManifest) -> bool {
    loaded.modules.values().any(is_placeholder_hash)
}
