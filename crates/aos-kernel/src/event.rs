use aos_wasm_abi::DomainEvent;

/// High-level kernel events processed by the deterministic stepper.
#[derive(Debug, Clone)]
pub enum KernelEvent {
    Reducer(ReducerEvent),
}

#[derive(Debug, Clone)]
pub struct ReducerEvent {
    pub event: DomainEvent,
}
