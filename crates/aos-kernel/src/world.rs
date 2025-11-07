use std::collections::HashMap;
use std::sync::Arc;

use aos_air_types::{Manifest, Name};
use aos_store::Store;
use aos_wasm_abi::{ABI_VERSION, CallContext, DomainEvent, ReducerInput, ReducerOutput};

use crate::effects::EffectManager;
use crate::error::KernelError;
use crate::event::{KernelEvent, ReducerEvent};
use crate::manifest::{LoadedManifest, ManifestLoader};
use crate::reducer::ReducerRegistry;
use crate::scheduler::Scheduler;

pub struct Kernel<S: Store> {
    manifest: Manifest,
    module_defs: HashMap<Name, aos_air_types::DefModule>,
    reducers: ReducerRegistry<S>,
    router: HashMap<String, Vec<Name>>,
    scheduler: Scheduler,
    effect_manager: EffectManager,
    reducer_state: HashMap<Name, Vec<u8>>,
}

pub struct KernelBuilder<S: Store> {
    store: Arc<S>,
}

impl<S: Store + 'static> KernelBuilder<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }

    pub fn from_manifest_path(
        self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<Kernel<S>, KernelError> {
        let loaded = ManifestLoader::load_from_path(&*self.store, path)?;
        Kernel::from_loaded_manifest(self.store, loaded)
    }
}

impl<S: Store + 'static> Kernel<S> {
    pub(crate) fn from_loaded_manifest(
        store: Arc<S>,
        loaded: LoadedManifest,
    ) -> Result<Self, KernelError> {
        let mut router = HashMap::new();
        if let Some(routing) = loaded.manifest.routing.as_ref() {
            for route in &routing.events {
                router
                    .entry(route.event.as_str().to_string())
                    .or_insert_with(Vec::new)
                    .push(route.reducer.clone());
            }
        }
        Ok(Self {
            manifest: loaded.manifest,
            module_defs: loaded.modules,
            reducers: ReducerRegistry::new(store)?,
            router,
            scheduler: Scheduler::default(),
            effect_manager: EffectManager::new(),
            reducer_state: HashMap::new(),
        })
    }

    pub fn enqueue_event(&mut self, event: KernelEvent) {
        self.scheduler.push(event);
    }

    pub fn submit_domain_event(&mut self, schema: impl Into<String>, value: Vec<u8>) {
        let event = DomainEvent::new(schema.into(), value);
        self.scheduler
            .push(KernelEvent::Reducer(ReducerEvent { event }));
    }

    pub fn tick(&mut self) -> Result<(), KernelError> {
        if let Some(event) = self.scheduler.pop() {
            match event {
                KernelEvent::Reducer(reducer_event) => self.handle_reducer_event(reducer_event)?,
            }
        }
        Ok(())
    }

    fn handle_reducer_event(&mut self, event: ReducerEvent) -> Result<(), KernelError> {
        let reducers = self
            .router
            .get(&event.event.schema)
            .cloned()
            .unwrap_or_default();
        for reducer_name in reducers {
            let module_def = self
                .module_defs
                .get(&reducer_name)
                .ok_or_else(|| KernelError::ReducerNotFound(reducer_name.clone()))?;
            self.reducers.ensure_loaded(&reducer_name, module_def)?;

            let input = ReducerInput {
                version: ABI_VERSION,
                state: self.reducer_state.get(&reducer_name).cloned(),
                event: event.event.clone(),
                ctx: CallContext::new(false, None),
            };
            let output = self.reducers.invoke(&reducer_name, &input)?;
            self.handle_reducer_output(reducer_name.clone(), output)?;
        }
        Ok(())
    }

    fn handle_reducer_output(
        &mut self,
        reducer_name: String,
        output: ReducerOutput,
    ) -> Result<(), KernelError> {
        match output.state {
            Some(state) => {
                self.reducer_state.insert(reducer_name.clone(), state);
            }
            None => {
                self.reducer_state.remove(&reducer_name);
            }
        }
        for event in output.domain_events {
            self.scheduler
                .push(KernelEvent::Reducer(ReducerEvent { event }));
        }
        if !output.effects.is_empty() {
            self.effect_manager
                .enqueue_reducer_effects(&output.effects)?;
        }
        Ok(())
    }

    pub fn drain_effects(&mut self) -> Vec<aos_effects::EffectIntent> {
        self.effect_manager.drain()
    }

    pub fn reducer_state(&self, reducer: &str) -> Option<&Vec<u8>> {
        self.reducer_state.get(reducer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use aos_air_types::{DefModule, HashRef, Manifest, ModuleAbi, ModuleKind, NamedRef, SchemaRef};
    use aos_store::MemStore;
    use aos_wasm_abi::{ReducerEffect, ReducerOutput};
    use wat::parse_str;

    #[test]
    fn kernel_runs_reducer_and_queues_effects() {
        let store = Arc::new(MemStore::new());
        let output = ReducerOutput {
            state: Some(vec![0xAA]),
            domain_events: vec![],
            effects: vec![ReducerEffect::new("timer.set", vec![0x01])],
            ann: None,
        };
        let output_bytes = output.encode().unwrap();
        let wasm_bytes = parse_str(&stub_reducer(&output_bytes));
        let wasm_bytes = wasm_bytes.expect("wat compile");
        let wasm_hash = store.put_blob(&wasm_bytes).unwrap();
        let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).unwrap();

        let module_def = DefModule {
            name: "com.acme/Reducer@1".into(),
            module_kind: ModuleKind::Reducer,
            wasm_hash: wasm_hash_ref,
            key_schema: None,
            abi: ModuleAbi { reducer: None },
        };

        let manifest = Manifest {
            schemas: vec![],
            modules: vec![NamedRef {
                name: module_def.name.clone(),
                hash: module_def.wasm_hash.clone(),
            }],
            plans: vec![],
            caps: vec![],
            policies: vec![],
            defaults: None,
            module_bindings: Default::default(),
            routing: Some(aos_air_types::Routing {
                events: vec![aos_air_types::RoutingEvent {
                    event: SchemaRef::new("com.acme/Start@1").unwrap(),
                    reducer: module_def.name.clone(),
                    key_field: None,
                }],
                inboxes: vec![],
            }),
            triggers: vec![],
        };

        let loaded = LoadedManifest {
            manifest,
            modules: HashMap::from([(module_def.name.clone(), module_def)]),
        };

        let mut kernel = Kernel::from_loaded_manifest(store, loaded).unwrap();
        kernel.submit_domain_event("com.acme/Start@1", vec![]);
        kernel.tick().unwrap();

        let effects = kernel.drain_effects();
        assert_eq!(effects.len(), 1);
        assert_eq!(
            kernel.reducer_state("com.acme/Reducer@1"),
            Some(&vec![0xAA])
        );
    }

    fn stub_reducer(output_bytes: &[u8]) -> String {
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
