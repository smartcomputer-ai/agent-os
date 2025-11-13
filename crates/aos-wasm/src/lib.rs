//! Deterministic WASM runner that executes reducer modules via the shared ABI.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use aos_wasm_abi::{ReducerInput, ReducerOutput};
use sha2::{Digest, Sha256};
use wasmtime::{Config, Engine, Linker, Module, Store};

const STEP_EXPORT: &str = "step";
const ALLOC_EXPORT: &str = "alloc";
const MEMORY_EXPORT: &str = "memory";

/// Deterministic runtime wrapper around Wasmtime.
pub struct ReducerRuntime {
    engine: Arc<Engine>,
    module_cache: Mutex<HashMap<ModuleKey, Arc<Module>>>,
}

impl ReducerRuntime {
    /// Build a runtime with deterministic configuration (no threads, no fuel, no debug info).
    pub fn new() -> Result<Self> {
        let mut cfg = Config::new();
        cfg.wasm_multi_value(true);
        cfg.wasm_threads(false);
        cfg.wasm_reference_types(true);
        cfg.consume_fuel(false);
        cfg.debug_info(false);
        cfg.cranelift_nan_canonicalization(true);
        let engine = Engine::new(&cfg)?;
        Ok(Self {
            engine: Arc::new(engine),
            module_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Compile a reducer WASM blob into a reusable Wasmtime module.
    pub fn compile(&self, wasm_bytes: &[u8]) -> Result<Module> {
        Module::new(&self.engine, wasm_bytes)
    }

    /// Execute an already-compiled module with the given ABI envelope.
    pub fn run_compiled(&self, module: &Module, input: &ReducerInput) -> Result<ReducerOutput> {
        let mut store = Store::new(&self.engine, ());
        let linker = Linker::new(&self.engine);
        let instance = linker.instantiate(&mut store, module)?;
        let memory = instance
            .get_memory(&mut store, MEMORY_EXPORT)
            .context("wasm export 'memory' not found")?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, ALLOC_EXPORT)
            .context("wasm export 'alloc' not found")?;
        let legacy_step =
            instance.get_typed_func::<(i32, i32), (i32, i32)>(&mut store, STEP_EXPORT);
        let modern_step = match legacy_step {
            Ok(step) => StepImpl::Legacy(step),
            Err(_) => StepImpl::Modern(
                instance
                    .get_typed_func::<(i32, i32, i32), ()>(&mut store, STEP_EXPORT)
                    .context("wasm export 'step' not found")?,
            ),
        };

        let input_bytes = input.encode()?;
        let input_len = i32::try_from(input_bytes.len()).context("input too large for wasm32")?;

        let input_ptr = alloc.call(&mut store, input_len)?;
        memory.write(&mut store, input_ptr as usize, &input_bytes)?;

        let output = match modern_step {
            StepImpl::Legacy(step) => {
                let (out_ptr, out_len) = step.call(&mut store, (input_ptr, input_len))?;
                let output_len = usize::try_from(out_len).context("negative output length")?;
                let mut output = vec![0u8; output_len];
                memory.read(&mut store, out_ptr as usize, &mut output)?;
                output
            }
            StepImpl::Modern(step) => {
                let result_ptr = alloc.call(&mut store, 8)?;
                step.call(&mut store, (result_ptr, input_ptr, input_len))?;
                let mut result_buf = [0u8; 8];
                memory.read(&mut store, result_ptr as usize, &mut result_buf)?;
                let out_ptr = i32::from_le_bytes([
                    result_buf[0],
                    result_buf[1],
                    result_buf[2],
                    result_buf[3],
                ]);
                let out_len = i32::from_le_bytes([
                    result_buf[4],
                    result_buf[5],
                    result_buf[6],
                    result_buf[7],
                ]);
                let output_len = usize::try_from(out_len).context("negative output length")?;
                let mut output = vec![0u8; output_len];
                memory.read(&mut store, out_ptr as usize, &mut output)?;
                output
            }
        };

        let reducer_output = ReducerOutput::decode(&output)?;
        Ok(reducer_output)
    }

    /// Execute a reducer WASM module with the given ABI envelope (compiles each time).
    pub fn run(&self, wasm_bytes: &[u8], input: &ReducerInput) -> Result<ReducerOutput> {
        let module = self.module_from_cache(wasm_bytes)?;
        self.run_compiled(&module, input)
    }

    fn module_from_cache(&self, wasm_bytes: &[u8]) -> Result<Arc<Module>> {
        let key = ModuleKey::from_bytes(wasm_bytes);
        if let Some(existing) = self
            .module_cache
            .lock()
            .expect("module cache poisoned")
            .get(&key)
            .cloned()
        {
            return Ok(existing);
        }

        let compiled = Arc::new(self.compile(wasm_bytes)?);
        let mut cache = self
            .module_cache
            .lock()
            .expect("module cache poisoned");
        let entry = cache.entry(key).or_insert_with(|| compiled.clone());
        Ok(entry.clone())
    }
}

enum StepImpl {
    Legacy(wasmtime::TypedFunc<(i32, i32), (i32, i32)>),
    Modern(wasmtime::TypedFunc<(i32, i32, i32), ()>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ModuleKey([u8; 32]);

impl ModuleKey {
    fn from_bytes(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest: [u8; 32] = hasher.finalize().into();
        Self(digest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_wasm_abi::{ABI_VERSION, CallContext, DomainEvent, ReducerEffect};
    #[test]
    fn reducer_round_trip_with_stub_module() {
        let runtime = ReducerRuntime::new().unwrap();
        let expected_output = ReducerOutput {
            state: None,
            domain_events: vec![DomainEvent::new("com.acme/Event@1", vec![0x10])],
            effects: vec![ReducerEffect::new("timer.set", vec![0x42])],
            ann: None,
        };
        let expected_bytes = expected_output.encode().unwrap();
        let wat = build_stub_module(&expected_bytes);
        let wasm_bytes = wat::parse_str(&wat).unwrap();

        let input = ReducerInput {
            version: ABI_VERSION,
            state: Some(vec![0xde, 0xad]),
            event: DomainEvent::new("com.acme/Event@1", vec![0x01]),
            ctx: CallContext::new(false, None),
        };

        let output = runtime.run(&wasm_bytes, &input).unwrap();
        assert_eq!(output, expected_output);
    }

    #[test]
    fn run_reuses_compiled_module() {
        let runtime = ReducerRuntime::new().unwrap();
        let expected_output = ReducerOutput {
            state: None,
            domain_events: vec![DomainEvent::new("demo/Event@1", vec![])],
            effects: Vec::new(),
            ann: None,
        };
        let expected_bytes = expected_output.encode().unwrap();
        let wasm_bytes = wat::parse_str(&build_stub_module(&expected_bytes)).unwrap();
        let input = ReducerInput {
            version: ABI_VERSION,
            state: None,
            event: DomainEvent::new("demo/Event@1", vec![]),
            ctx: CallContext::new(false, None),
        };

        runtime.run(&wasm_bytes, &input).unwrap();
        let first_cache_size = runtime.cached_module_count();
        runtime.run(&wasm_bytes, &input).unwrap();
        let second_cache_size = runtime.cached_module_count();
        assert_eq!(first_cache_size, 1);
        assert_eq!(second_cache_size, 1);
    }

    fn build_stub_module(output_bytes: &[u8]) -> String {
        let data_literal = output_bytes
            .iter()
            .map(|b| format!("\\{:02x}", b))
            .collect::<String>();
        let len = output_bytes.len();
        format!(
            r#"(module
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const {len}))
  (data (i32.const 0) "{data}")
  (func (export "alloc") (param i32) (result i32)
    (local $old i32)
    global.get $heap
    local.tee $old
    local.get 0
    i32.add
    global.set $heap
    local.get $old)
  (func (export "step") (param i32 i32) (result i32 i32)
    (i32.const 0)
    (i32.const {len}))
)"#,
            len = len,
            data = data_literal
        )
    }
}

#[cfg(test)]
impl ReducerRuntime {
    fn cached_module_count(&self) -> usize {
        self.module_cache
            .lock()
            .expect("module cache poisoned")
            .len()
    }
}
