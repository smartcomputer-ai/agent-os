use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use aos_air_types::{DefModule, Name};
use aos_cbor::Hash;
use aos_wasm::WorkflowRuntime;
use aos_wasm_abi::{WorkflowInput, WorkflowOutput};

use crate::Store;
use crate::error::KernelError;
use crate::module_runtime::wasm_hash_ref;

pub(super) struct WasmWorkflowBackend<S: Store> {
    store: Arc<S>,
    runtime: WorkflowRuntime,
    modules: HashMap<Name, WasmWorkflowModule>,
}

struct WasmWorkflowModule {
    wasm_hash: String,
    instance_pre: Arc<wasmtime::InstancePre<()>>,
}

impl<S: Store> WasmWorkflowBackend<S> {
    pub(super) fn new(
        store: Arc<S>,
        module_cache_dir: Option<PathBuf>,
    ) -> Result<Self, KernelError> {
        Ok(Self {
            store,
            runtime: WorkflowRuntime::new_with_disk_cache(module_cache_dir)
                .map_err(KernelError::Wasm)?,
            modules: HashMap::new(),
        })
    }

    pub(super) fn ensure_loaded(
        &mut self,
        workflow_name: &str,
        module_def: &DefModule,
    ) -> Result<String, KernelError> {
        let wasm_hash_ref = wasm_hash_ref(workflow_name, module_def)?;
        if let Some(existing) = self.modules.get(workflow_name) {
            if existing.wasm_hash == wasm_hash_ref.as_str() {
                return Ok(existing.wasm_hash.clone());
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
        let wasm_hash = wasm_hash_ref.to_string();
        self.modules.insert(
            workflow_name.to_string(),
            WasmWorkflowModule {
                wasm_hash: wasm_hash.clone(),
                instance_pre,
            },
        );
        Ok(wasm_hash)
    }

    pub(super) fn invoke_export(
        &self,
        workflow_name: &str,
        wasm_hash: &str,
        export_name: &str,
        input: &WorkflowInput,
    ) -> Result<WorkflowOutput, KernelError> {
        let module = self
            .modules
            .get(workflow_name)
            .filter(|module| module.wasm_hash == wasm_hash)
            .ok_or_else(|| KernelError::WorkflowNotFound(workflow_name.to_string()))?;
        let output = self
            .runtime
            .run_precompiled_export(module.instance_pre.as_ref(), export_name, input)
            .map_err(KernelError::Wasm)?;
        Ok(output)
    }
}
