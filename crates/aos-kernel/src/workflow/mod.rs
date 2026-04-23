use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use aos_air_types::{DefModule, ModuleRuntime, Name};
use aos_wasm_abi::{WorkflowInput, WorkflowOutput};

use crate::Store;
use crate::error::KernelError;

mod builtin;
mod wasm;

pub struct WorkflowRegistry<S: Store> {
    wasm: wasm::WasmWorkflowBackend<S>,
    workflows: HashMap<Name, LoadedWorkflow>,
}

struct LoadedWorkflow {
    module_name: Name,
    runtime: LoadedWorkflowRuntime,
}

enum LoadedWorkflowRuntime {
    Wasm { wasm_hash: String },
    Builtin,
}

impl<S: Store> WorkflowRegistry<S> {
    pub fn new(store: Arc<S>, module_cache_dir: Option<PathBuf>) -> Result<Self, KernelError> {
        Ok(Self {
            wasm: wasm::WasmWorkflowBackend::new(store, module_cache_dir)?,
            workflows: HashMap::new(),
        })
    }

    pub fn ensure_loaded(
        &mut self,
        workflow_name: &str,
        module_def: &DefModule,
    ) -> Result<(), KernelError> {
        match &module_def.runtime {
            ModuleRuntime::Wasm { .. } => {
                let wasm_hash = self.wasm.ensure_loaded(workflow_name, module_def)?;
                self.workflows.insert(
                    workflow_name.to_string(),
                    LoadedWorkflow {
                        module_name: module_def.name.clone(),
                        runtime: LoadedWorkflowRuntime::Wasm { wasm_hash },
                    },
                );
                Ok(())
            }
            ModuleRuntime::Builtin {} => {
                self.workflows.insert(
                    workflow_name.to_string(),
                    LoadedWorkflow {
                        module_name: module_def.name.clone(),
                        runtime: LoadedWorkflowRuntime::Builtin,
                    },
                );
                Ok(())
            }
            ModuleRuntime::Python { .. } => Err(KernelError::Manifest(format!(
                "workflow module '{}' uses unsupported python runtime",
                module_def.name
            ))),
        }
    }

    pub fn invoke(
        &self,
        workflow_name: &str,
        input: &WorkflowInput,
    ) -> Result<WorkflowOutput, KernelError> {
        self.invoke_export(workflow_name, "step", input)
    }

    pub fn invoke_export(
        &self,
        workflow_name: &str,
        export_name: &str,
        input: &WorkflowInput,
    ) -> Result<WorkflowOutput, KernelError> {
        let workflow = self
            .workflows
            .get(workflow_name)
            .ok_or_else(|| KernelError::WorkflowNotFound(workflow_name.to_string()))?;
        match &workflow.runtime {
            LoadedWorkflowRuntime::Wasm { wasm_hash } => {
                self.wasm
                    .invoke_export(workflow_name, wasm_hash, export_name, input)
            }
            LoadedWorkflowRuntime::Builtin => {
                builtin::invoke_builtin_workflow(&workflow.module_name, export_name, input)
            }
        }
    }
}
