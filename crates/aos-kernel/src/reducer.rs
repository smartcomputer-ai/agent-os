use std::collections::HashMap;
use std::sync::Arc;

use aos_air_types::{DefModule, Name};
use aos_cbor::Hash;
use aos_store::Store;
use aos_wasm::ReducerRuntime;
use aos_wasm_abi::{ReducerInput, ReducerOutput};

use crate::error::KernelError;

pub struct ReducerRegistry<S: Store> {
    store: Arc<S>,
    runtime: ReducerRuntime,
    modules: HashMap<Name, ReducerModule>,
}

struct ReducerModule {
    module: Arc<wasmtime::Module>,
}

impl<S: Store> ReducerRegistry<S> {
    pub fn new(store: Arc<S>) -> Result<Self, KernelError> {
        Ok(Self {
            store,
            runtime: ReducerRuntime::new().map_err(KernelError::Wasm)?,
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
            .compile(&bytes)
            .map_err(KernelError::Wasm)?;
        self.modules.insert(
            name.to_string(),
            ReducerModule {
                module: Arc::new(compiled),
            },
        );
        Ok(())
    }

    pub fn invoke(&self, name: &str, input: &ReducerInput) -> Result<ReducerOutput, KernelError> {
        let module = self
            .modules
            .get(name)
            .ok_or_else(|| KernelError::ReducerNotFound(name.to_string()))?;
        let output = self
            .runtime
            .run_compiled(&module.module, input)
            .map_err(KernelError::Wasm)?;
        Ok(output)
    }
}
