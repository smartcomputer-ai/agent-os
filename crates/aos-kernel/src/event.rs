use aos_wasm_abi::DomainEvent;

/// High-level kernel events processed by the deterministic stepper.
#[derive(Debug, Clone)]
pub enum KernelEvent {
    Workflow(WorkflowEvent),
}

#[derive(Debug, Clone)]
pub struct IngressStamp {
    pub now_ns: u64,
    pub logical_now_ns: u64,
    pub entropy: Vec<u8>,
    pub journal_height: u64,
    pub manifest_hash: String,
}

#[derive(Debug, Clone)]
pub struct WorkflowEvent {
    pub workflow: String,
    pub event: DomainEvent,
    pub stamp: IngressStamp,
}
