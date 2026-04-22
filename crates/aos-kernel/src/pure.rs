use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::Store;
use crate::module_runtime::wasm_hash_ref;
use aos_air_types::{DefModule, Name};
use aos_cbor::Hash;
use aos_wasm::PureRuntime;
use aos_wasm_abi::{PureInput, PureOutput};

use crate::error::KernelError;

pub struct PureRegistry<S: Store> {
    store: Arc<S>,
    runtime: PureRuntime,
    modules: HashMap<Name, PureModule>,
}

struct PureModule {
    wasm_hash: String,
    instance_pre: Arc<wasmtime::InstancePre<()>>,
}

impl<S: Store> PureRegistry<S> {
    pub fn new(store: Arc<S>, module_cache_dir: Option<PathBuf>) -> Result<Self, KernelError> {
        Ok(Self {
            store,
            runtime: PureRuntime::new_with_disk_cache(module_cache_dir)
                .map_err(KernelError::Wasm)?,
            modules: HashMap::new(),
        })
    }

    pub fn ensure_loaded(&mut self, name: &str, module_def: &DefModule) -> Result<(), KernelError> {
        let wasm_hash_ref = wasm_hash_ref(name, module_def)?;
        if let Some(existing) = self.modules.get(name) {
            if existing.wasm_hash == wasm_hash_ref.as_str() {
                return Ok(());
            }
        }
        let wasm_hash = Hash::from_hex_str(wasm_hash_ref.as_str())
            .map_err(|err| KernelError::Manifest(err.to_string()))?;
        let bytes: Vec<u8> = self.store.get_blob(wasm_hash)?;
        let compiled = self
            .runtime
            .cached_module(&bytes)
            .map_err(KernelError::Wasm)?;
        let instance_pre = self
            .runtime
            .preinstantiate(compiled.as_ref())
            .map_err(KernelError::Wasm)?;
        self.modules.insert(
            name.to_string(),
            PureModule {
                wasm_hash: wasm_hash_ref.to_string(),
                instance_pre,
            },
        );
        Ok(())
    }

    pub fn invoke(&self, name: &str, input: &PureInput) -> Result<PureOutput, KernelError> {
        let module = self
            .modules
            .get(name)
            .ok_or_else(|| KernelError::PureNotFound(name.to_string()))?;
        let output = self
            .runtime
            .run_precompiled(module.instance_pre.as_ref(), input)
            .map_err(KernelError::Wasm)?;
        Ok(output)
    }
}
