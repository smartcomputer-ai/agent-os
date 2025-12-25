use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use aos_air_types::{DefModule, Name};
use aos_cbor::Hash;
use aos_store::Store;
use aos_wasm::PureRuntime;
use aos_wasm_abi::{PureInput, PureOutput};

use crate::error::KernelError;

pub struct PureRegistry<S: Store> {
    store: Arc<S>,
    runtime: PureRuntime,
    modules: HashMap<Name, PureModule>,
}

struct PureModule {
    module: Arc<wasmtime::Module>,
}

impl<S: Store> PureRegistry<S> {
    pub fn new(store: Arc<S>, module_cache_dir: Option<PathBuf>) -> Result<Self, KernelError> {
        Ok(Self {
            store,
            runtime: PureRuntime::new_with_disk_cache(module_cache_dir).map_err(KernelError::Wasm)?,
            modules: HashMap::new(),
        })
    }

    pub fn ensure_loaded(&mut self, name: &str, module_def: &DefModule) -> Result<(), KernelError> {
        if self.modules.contains_key(name) {
            return Ok(());
        }
        let wasm_hash = Hash::from_hex_str(module_def.wasm_hash.as_str())
            .map_err(|err| KernelError::Manifest(err.to_string()))?;
        let bytes: Vec<u8> = self.store.get_blob(wasm_hash)?;
        let compiled = self
            .runtime
            .cached_module(&bytes)
            .map_err(KernelError::Wasm)?;
        self.modules
            .insert(name.to_string(), PureModule { module: compiled });
        Ok(())
    }

    pub fn invoke(&self, name: &str, input: &PureInput) -> Result<PureOutput, KernelError> {
        let module = self
            .modules
            .get(name)
            .ok_or_else(|| KernelError::PureNotFound(name.to_string()))?;
        let output = self
            .runtime
            .run_compiled(&module.module, input)
            .map_err(KernelError::Wasm)?;
        Ok(output)
    }
}
