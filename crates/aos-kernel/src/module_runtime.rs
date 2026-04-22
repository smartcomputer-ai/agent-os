use aos_air_types::{DefModule, HashRef, ModuleRuntime, WasmArtifact};

use crate::error::KernelError;

pub(crate) fn wasm_hash_ref<'a>(
    module_name: &str,
    module_def: &'a DefModule,
) -> Result<&'a HashRef, KernelError> {
    match &module_def.runtime {
        ModuleRuntime::Wasm {
            artifact: WasmArtifact::WasmModule { hash },
        } => Ok(hash),
        _ => Err(KernelError::Manifest(format!(
            "module '{module_name}' is not a wasm module"
        ))),
    }
}

pub(crate) fn wasm_hash_string(
    module_name: &str,
    module_def: &DefModule,
) -> Result<String, KernelError> {
    Ok(wasm_hash_ref(module_name, module_def)?.as_str().to_string())
}

pub(crate) fn is_wasm_module(module_def: &DefModule) -> bool {
    matches!(module_def.runtime, ModuleRuntime::Wasm { .. })
}
