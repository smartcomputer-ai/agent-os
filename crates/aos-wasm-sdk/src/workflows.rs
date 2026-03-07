use alloc::string::{String, ToString};
use alloc::vec::Vec;
use aos_wasm_abi::{
    AbiDecodeError, AbiEncodeError, DomainEvent as AbiDomainEvent, WorkflowContext,
    WorkflowEffect as AbiWorkflowEffect, WorkflowInput, WorkflowOutput,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{read_input, write_back};
use serde_cbor::Value;

/// Trait implemented by every workflow.
pub trait Workflow: Default {
    /// Durable workflow state; persisted via canonical CBOR.
    type State: Serialize + DeserializeOwned + Default;

    /// Event family consumed by this workflow.
    type Event: DeserializeOwned;

    /// Optional annotation payload for observability.
    type Ann: Serialize;

    /// Core workflow logic.
    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError>;
}

/// Workflow execution context passed to `Workflow::reduce`.
pub struct WorkflowCtx<S, A = Value> {
    pub state: S,
    ann: Option<A>,
    context: Option<WorkflowContext>,
    workflow_name: &'static str,
    domain_events: Vec<PendingEvent>,
    effects: Vec<EmittedEffect>,
}

impl<S, A> WorkflowCtx<S, A> {
    fn new(state: S, ctx: Option<WorkflowContext>, workflow_name: &'static str) -> Self {
        Self {
            state,
            ann: None,
            context: ctx,
            workflow_name,
            domain_events: Vec::new(),
            effects: Vec::new(),
        }
    }

    /// Optional cell key provided by the kernel.
    pub fn key(&self) -> Option<&[u8]> {
        self.context.as_ref().and_then(|ctx| ctx.key.as_deref())
    }

    /// Return the key and fail deterministically if the workflow is keyed but no key was supplied.
    pub fn key_required(&self) -> Result<&[u8], ReduceError> {
        self.context
            .as_ref()
            .and_then(|ctx| ctx.key.as_deref())
            .ok_or(ReduceError::new("missing workflow key"))
    }

    /// Return the key interpreted as UTF-8 text (fails if absent or invalid UTF-8).
    pub fn key_text(&self) -> Result<&str, ReduceError> {
        let bytes = self.key_required()?;
        core::str::from_utf8(bytes).map_err(|_| ReduceError::new("key not utf8"))
    }

    /// Enforce that the provided bytes exactly match the workflow key (when present).
    pub fn ensure_key_eq(&self, expected: &[u8]) -> Result<(), ReduceError> {
        match self.context.as_ref().and_then(|ctx| ctx.key.as_deref()) {
            Some(actual) if actual == expected => Ok(()),
            Some(_) => Err(ReduceError::new("key mismatch")),
            None => Err(ReduceError::new("missing workflow key")),
        }
    }

    /// Name of the workflow type (for diagnostics).
    pub fn workflow_name(&self) -> &'static str {
        self.workflow_name
    }

    /// Access the full call context supplied by the kernel.
    pub fn context(&self) -> Option<&WorkflowContext> {
        self.context.as_ref()
    }

    /// Monotonic kernel time (ns) at ingress.
    pub fn logical_now_ns(&self) -> Option<u64> {
        self.context.as_ref().map(|ctx| ctx.logical_now_ns)
    }

    /// Wall clock (ns) sampled at ingress.
    pub fn now_ns(&self) -> Option<u64> {
        self.context.as_ref().map(|ctx| ctx.now_ns)
    }

    /// Journal height for this invocation.
    pub fn journal_height(&self) -> Option<u64> {
        self.context.as_ref().map(|ctx| ctx.journal_height)
    }

    /// Entropy bytes sampled at ingress.
    pub fn entropy(&self) -> Option<&[u8]> {
        self.context.as_ref().map(|ctx| ctx.entropy.as_slice())
    }

    /// Hash of the canonical event envelope.
    pub fn event_hash(&self) -> Option<&str> {
        self.context.as_ref().map(|ctx| ctx.event_hash.as_str())
    }

    /// Manifest hash for the active world.
    pub fn manifest_hash(&self) -> Option<&str> {
        self.context.as_ref().map(|ctx| ctx.manifest_hash.as_str())
    }

    /// Workflow module name string.
    pub fn workflow_module(&self) -> Option<&str> {
        self.context.as_ref().map(|ctx| ctx.workflow.as_str())
    }

    /// Whether the workflow is operating in keyed mode.
    pub fn cell_mode(&self) -> Option<bool> {
        self.context.as_ref().map(|ctx| ctx.cell_mode)
    }

    /// Attach structured annotations.
    pub fn annotate(&mut self, ann: A) {
        self.ann = Some(ann);
    }

    /// Access the current annotation.
    pub fn annotation(&self) -> Option<&A> {
        self.ann.as_ref()
    }

    /// Builder for a domain intent/event.
    pub fn intent(&mut self, schema: &'static str) -> IntentBuilder<'_, S, A> {
        IntentBuilder::new(self, schema)
    }

    /// Emit micro-effects.
    pub fn effects(&mut self) -> Effects<'_, S, A> {
        Effects { ctx: self }
    }

    fn push_event(&mut self, event: PendingEvent) {
        self.domain_events.push(event);
    }

    fn emit_effect(&mut self, effect: EmittedEffect) {
        self.effects.push(effect);
    }

    fn finish(self) -> Result<WorkflowOutput, StepError>
    where
        A: Serialize,
        S: Serialize,
    {
        let state_bytes = serde_cbor::to_vec(&self.state).map_err(StepError::StateEncode)?;
        let domain_events = self
            .domain_events
            .into_iter()
            .map(|evt| evt.into_abi())
            .collect();
        let effects = self.effects.into_iter().map(|eff| eff.into_abi()).collect();
        let ann_bytes = match self.ann {
            Some(ann) => Some(serde_cbor::to_vec(&ann).map_err(StepError::AnnEncode)?),
            None => None,
        };
        Ok(WorkflowOutput {
            state: Some(state_bytes),
            domain_events,
            effects,
            ann: ann_bytes,
        })
    }

    #[track_caller]
    fn trap(&self, err: StepError) -> ! {
        err.trap(self.workflow_name)
    }
}

/// Domain intent builder.
pub struct IntentBuilder<'ctx, S, A> {
    ctx: &'ctx mut WorkflowCtx<S, A>,
    schema: &'static str,
    key: Option<Vec<u8>>,
    payload: IntentPayload,
}

impl<'ctx, S, A> IntentBuilder<'ctx, S, A> {
    fn new(ctx: &'ctx mut WorkflowCtx<S, A>, schema: &'static str) -> Self {
        Self {
            ctx,
            schema,
            key: None,
            payload: IntentPayload::Unset,
        }
    }

    /// Attach a partitioning key.
    pub fn key_bytes(mut self, key: &[u8]) -> Self {
        self.key = Some(key.to_vec());
        self
    }

    /// Provide a structured payload (serialized as canonical CBOR).
    pub fn payload<T: Serialize>(mut self, value: &T) -> Self {
        match serde_cbor::to_vec(value) {
            Ok(bytes) => self.payload = IntentPayload::Ready(bytes),
            Err(err) => self.ctx.trap(StepError::IntentPayload(err)),
        }
        self
    }

    /// Finalize and enqueue the intent.
    pub fn send(self) {
        let payload = match self.payload {
            IntentPayload::Ready(bytes) => bytes,
            IntentPayload::Unset => match serde_cbor::to_vec(&Value::Null) {
                Ok(bytes) => bytes,
                Err(err) => self.ctx.trap(StepError::IntentPayload(err)),
            },
        };
        self.ctx.push_event(PendingEvent {
            schema: self.schema,
            value: payload,
            key: self.key,
        });
    }
}

enum IntentPayload {
    Unset,
    Ready(Vec<u8>),
}

/// Effect emission helper enforcing the single-effect rule.
pub struct Effects<'ctx, S, A> {
    ctx: &'ctx mut WorkflowCtx<S, A>,
}

impl<'ctx, S, A> Effects<'ctx, S, A> {
    /// Enter the namespaced sys-effect authoring surface.
    pub fn sys(&mut self) -> crate::SysEffects<'_, 'ctx, S, A> {
        crate::SysEffects::new(self)
    }

    /// Escape hatch for future micro-effects.
    pub fn emit_raw(
        &mut self,
        kind: &'static str,
        params: &impl Serialize,
        cap_slot: Option<&str>,
    ) {
        self.emit_raw_with_refs(kind, params, cap_slot, None, None);
    }

    /// Emit a micro-effect with an explicit idempotency key (32 bytes).
    pub fn emit_raw_with_idempotency(
        &mut self,
        kind: &'static str,
        params: &impl Serialize,
        cap_slot: Option<&str>,
        idempotency_key: Option<&[u8]>,
    ) {
        self.emit_raw_with_refs(kind, params, cap_slot, idempotency_key, None);
    }

    /// Emit a micro-effect with an explicit issuer reference echoed in continuations.
    pub fn emit_raw_with_issuer_ref(
        &mut self,
        kind: &'static str,
        params: &impl Serialize,
        cap_slot: Option<&str>,
        issuer_ref: Option<&str>,
    ) {
        self.emit_raw_with_refs(kind, params, cap_slot, None, issuer_ref);
    }

    fn emit_raw_with_refs(
        &mut self,
        kind: &'static str,
        params: &impl Serialize,
        cap_slot: Option<&str>,
        idempotency_key: Option<&[u8]>,
        issuer_ref: Option<&str>,
    ) {
        let payload = match serde_cbor::to_vec(params) {
            Ok(bytes) => bytes,
            Err(err) => self.ctx.trap(StepError::EffectPayload(err)),
        };
        let key = idempotency_key.map(|bytes| bytes.to_vec());
        self.ctx.emit_effect(EmittedEffect {
            kind,
            params: payload,
            cap_slot: cap_slot.map(|s| s.to_string()),
            idempotency_key: key,
            issuer_ref: issuer_ref.map(|value| value.to_string()),
        });
    }

    /// Emit and register a durable handle for a workflow-origin effect intent.
    pub fn emit_tracked(
        &mut self,
        pending: &mut crate::PendingEffects,
        kind: &'static str,
        params: &impl Serialize,
        cap_slot: Option<&str>,
    ) -> crate::PendingEffect {
        self.emit_tracked_with_issuer_ref(pending, kind, params, cap_slot, None)
    }

    /// Emit and register a durable handle with an issuer reference echoed in continuations.
    pub fn emit_tracked_with_issuer_ref(
        &mut self,
        pending: &mut crate::PendingEffects,
        kind: &'static str,
        params: &impl Serialize,
        cap_slot: Option<&str>,
        issuer_ref: Option<&str>,
    ) -> crate::PendingEffect {
        let encoded = match crate::encode_effect_params(params) {
            Ok(value) => value,
            Err(err) => self.ctx.trap(StepError::EffectPayload(err)),
        };
        let pending_effect = crate::PendingEffect::new(
            kind,
            encoded.params_hash.clone(),
            cap_slot.map(|slot| slot.to_string()),
            self.ctx.now_ns().unwrap_or_default(),
        )
        .with_issuer_ref_opt(issuer_ref.map(|value| value.to_string()));
        pending.insert(pending_effect.clone());
        self.ctx.emit_effect(EmittedEffect {
            kind,
            params: encoded.cbor,
            cap_slot: cap_slot.map(|s| s.to_string()),
            idempotency_key: None,
            issuer_ref: issuer_ref.map(|value| value.to_string()),
        });
        pending_effect
    }
}

struct PendingEvent {
    schema: &'static str,
    value: Vec<u8>,
    key: Option<Vec<u8>>,
}

impl PendingEvent {
    fn into_abi(self) -> AbiDomainEvent {
        AbiDomainEvent {
            schema: self.schema.to_string(),
            value: self.value,
            key: self.key,
        }
    }
}

struct EmittedEffect {
    kind: &'static str,
    params: Vec<u8>,
    cap_slot: Option<String>,
    idempotency_key: Option<Vec<u8>>,
    issuer_ref: Option<String>,
}

impl EmittedEffect {
    fn into_abi(self) -> AbiWorkflowEffect {
        let mut eff = AbiWorkflowEffect::new(self.kind, self.params);
        if let Some(slot) = self.cap_slot {
            eff.cap_slot = Some(slot);
        }
        if let Some(issuer_ref) = self.issuer_ref {
            eff.issuer_ref = Some(issuer_ref);
        }
        if let Some(key) = self.idempotency_key {
            eff.idempotency_key = Some(key);
        }
        eff
    }
}

/// Deterministic workflow failure.
#[derive(Debug, Clone, Copy)]
pub struct ReduceError {
    msg: &'static str,
}

impl ReduceError {
    pub const fn new(msg: &'static str) -> Self {
        Self { msg }
    }

    pub fn message(&self) -> &'static str {
        self.msg
    }
}

impl core::fmt::Display for ReduceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.msg)
    }
}

/// Errors surfaced while running the workflow entrypoint.
#[derive(Debug)]
pub enum StepError {
    AbiDecode(AbiDecodeError),
    StateDecode(serde_cbor::Error),
    EventDecode(serde_cbor::Error),
    CtxDecode(serde_cbor::Error),
    StateEncode(serde_cbor::Error),
    AnnEncode(serde_cbor::Error),
    OutputEncode(AbiEncodeError),
    IntentPayload(serde_cbor::Error),
    EffectPayload(serde_cbor::Error),
    Reduce(&'static str),
}

impl StepError {
    #[cold]
    pub fn trap(self, workflow: &'static str) -> ! {
        panic!("aos-wasm-sdk({workflow}): {self}");
    }
}

impl core::fmt::Display for StepError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StepError::AbiDecode(err) => write!(f, "abi decode failed: {err}"),
            StepError::StateDecode(err) => write!(f, "state decode failed: {err}"),
            StepError::EventDecode(err) => write!(f, "event decode failed: {err}"),
            StepError::CtxDecode(err) => write!(f, "context decode failed: {err}"),
            StepError::StateEncode(err) => write!(f, "state encode failed: {err}"),
            StepError::AnnEncode(err) => write!(f, "annotation encode failed: {err}"),
            StepError::OutputEncode(err) => write!(f, "output encode failed: {err}"),
            StepError::IntentPayload(err) => write!(f, "intent payload encode failed: {err}"),
            StepError::EffectPayload(err) => write!(f, "effect payload encode failed: {err}"),
            StepError::Reduce(msg) => write!(f, "reduce error: {msg}"),
        }
    }
}

/// Execute a workflow against a workflow input envelope (used by tests).
pub fn step_bytes<R: Workflow>(input: &[u8]) -> Result<Vec<u8>, StepError>
where
    R::State: Serialize,
    R::Ann: Serialize,
{
    let env = WorkflowInput::decode(input).map_err(StepError::AbiDecode)?;
    run_workflow::<R>(env)
}

fn run_workflow<R: Workflow>(input: WorkflowInput) -> Result<Vec<u8>, StepError>
where
    R::State: Serialize,
    R::Ann: Serialize,
{
    let workflow_name = core::any::type_name::<R>();
    let state = match input.state {
        Some(bytes) => serde_cbor::from_slice(&bytes).map_err(StepError::StateDecode)?,
        None => R::State::default(),
    };
    let event = serde_cbor::from_slice(&input.event.value).map_err(StepError::EventDecode)?;
    let context = match &input.ctx {
        Some(bytes) => Some(WorkflowContext::decode(bytes).map_err(StepError::CtxDecode)?),
        None => None,
    };
    let mut ctx = WorkflowCtx::new(state, context, workflow_name);
    let mut workflow = R::default();
    workflow
        .reduce(event, &mut ctx)
        .map_err(|err| StepError::Reduce(err.message()))?;
    let output = ctx.finish()?;
    output.encode().map_err(StepError::OutputEncode)
}

/// Entry helper for exported `step`.
pub fn dispatch_workflow<R: Workflow>(ptr: i32, len: i32) -> (i32, i32)
where
    R::State: Serialize,
    R::Ann: Serialize,
{
    let workflow_name = core::any::type_name::<R>();
    let bytes = unsafe { read_input(ptr, len) };
    match step_bytes::<R>(bytes) {
        Ok(output) => write_back(&output),
        Err(err) => err.trap(workflow_name),
    }
}

/// Macro wiring the workflow entrypoints.
#[macro_export]
macro_rules! aos_workflow {
    ($ty:ty) => {
        #[cfg_attr(target_arch = "wasm32", unsafe(export_name = "alloc"))]
        pub extern "C" fn aos_wasm_alloc(len: i32) -> i32 {
            $crate::exported_alloc(len)
        }

        #[cfg_attr(target_arch = "wasm32", unsafe(export_name = "step"))]
        pub extern "C" fn aos_wasm_step(ptr: i32, len: i32) -> (i32, i32) {
            $crate::dispatch_workflow::<$ty>(ptr, len)
        }
    };
}

/// Helper macro to apply AIR's canonical variant tagging (`$tag`/`$value`) to an enum.
///
/// This avoids repeating `#[serde(tag = "$tag", content = "$value")]` on every workflow
/// event/state enum. Usage:
///
/// ```
/// use serde::{Serialize, Deserialize};
/// use aos_wasm_sdk::aos_variant;
///
/// aos_variant! {
///     #[derive(Serialize, Deserialize)]
///     pub enum Event {
///         Start { target: u64 },
///         Tick,
///     }
/// }
/// ```
#[macro_export]
macro_rules! aos_variant {
    ($(#[$meta:meta])* $vis:vis enum $name:ident $($rest:tt)*) => {
        $(#[$meta])*
        #[serde(tag = "$tag", content = "$value")]
        $vis enum $name $($rest)*
    };
}

/// Helper macro to define an enum that deserializes from AIR's canonical tagged
/// variant form (`$tag`/`$value`).
#[macro_export]
macro_rules! aos_event_union {
    ($(#[$meta:meta])* $vis:vis enum $name:ident { $($variant:ident ( $ty:ty )),+ $(,)? }) => {
        $(#[$meta])*
        $vis enum $name {
            $($variant($ty)),+
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = serde_cbor::Value::deserialize(deserializer)?;
                let serde_cbor::Value::Map(map) = value else {
                    return Err(serde::de::Error::custom(
                        "event union expects canonical tagged object",
                    ));
                };
                let Some(serde_cbor::Value::Text(tag)) =
                    map.get(&serde_cbor::Value::Text("$tag".into()))
                else {
                    return Err(serde::de::Error::custom(
                        "event union payload missing '$tag'",
                    ));
                };
                let inner = map
                    .get(&serde_cbor::Value::Text("$value".into()))
                    .cloned()
                    .unwrap_or(serde_cbor::Value::Null);
                $(
                    let expected = stringify!($variant);
                    let expected_lower = expected.to_ascii_lowercase();
                    if tag == expected || tag == expected_lower.as_str() {
                        let decoded = serde_cbor::value::from_value::<$ty>(inner.clone())
                            .map_err(serde::de::Error::custom)?;
                        return Ok(Self::$variant(decoded));
                    }
                )+
                Err(serde::de::Error::custom("event union tag did not match any variant"))
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectReceiptEnvelope, PendingEffects, TimerSetParams};
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use aos_wasm_abi::{DomainEvent, WorkflowContext, WorkflowInput};
    use serde::{Deserialize, Serialize};

    fn context_bytes(workflow: &str) -> Vec<u8> {
        let ctx = WorkflowContext {
            now_ns: 1,
            logical_now_ns: 2,
            journal_height: 3,
            entropy: vec![0x11; 64],
            event_hash: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
            manifest_hash:
                "sha256:1111111111111111111111111111111111111111111111111111111111111111".into(),
            workflow: workflow.into(),
            key: None,
            cell_mode: false,
        };
        serde_cbor::to_vec(&ctx).expect("context bytes")
    }

    #[derive(Default)]
    struct TestWorkflow;

    #[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
    struct TestState {
        counter: u64,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum TestEvent {
        Increment(u64),
    }

    impl Workflow for TestWorkflow {
        type State = TestState;
        type Event = TestEvent;
        type Ann = Value;

        fn reduce(
            &mut self,
            event: Self::Event,
            ctx: &mut WorkflowCtx<Self::State>,
        ) -> Result<(), ReduceError> {
            match event {
                TestEvent::Increment(v) => {
                    ctx.state.counter += v;
                    let current = ctx.state.counter;
                    ctx.intent("com.acme/Test@1").payload(&current).send();
                }
            }
            Ok(())
        }
    }

    #[test]
    fn intent_builder_serializes_payload() {
        let input = WorkflowInput {
            version: aos_wasm_abi::ABI_VERSION,
            state: None,
            event: DomainEvent::new(
                "schema",
                serde_cbor::to_vec(&TestEvent::Increment(1)).unwrap(),
            ),
            ctx: Some(context_bytes("com.acme/TestWorkflow@1")),
        };
        let bytes = input.encode().unwrap();
        let output = step_bytes::<TestWorkflow>(&bytes).expect("step");
        let decoded = WorkflowOutput::decode(&output).expect("decode");
        assert_eq!(decoded.domain_events.len(), 1);
        let payload: u64 = serde_cbor::from_slice(&decoded.domain_events[0].value).unwrap();
        assert_eq!(payload, 1);
    }

    #[test]
    fn workflow_can_emit_multiple_effects() {
        #[derive(Default)]
        struct EffectWorkflow;

        #[derive(Default, Serialize, Deserialize)]
        struct EffectState;

        #[derive(Serialize, Deserialize)]
        struct EffectEvent;

        impl Workflow for EffectWorkflow {
            type State = EffectState;
            type Event = EffectEvent;
            type Ann = Value;

            fn reduce(
                &mut self,
                _event: Self::Event,
                ctx: &mut WorkflowCtx<Self::State>,
            ) -> Result<(), ReduceError> {
                let params = TimerSetParams {
                    deliver_at_ns: 42,
                    key: None,
                };
                ctx.effects().sys().timer_set(&params, "clock");
                ctx.effects().sys().timer_set(&params, "clock");
                Ok(())
            }
        }

        let input = WorkflowInput {
            version: aos_wasm_abi::ABI_VERSION,
            state: None,
            event: DomainEvent::new("schema", serde_cbor::to_vec(&EffectEvent).unwrap()),
            ctx: Some(context_bytes("com.acme/EffectWorkflow@1")),
        };
        let bytes = input.encode().unwrap();
        let output = step_bytes::<EffectWorkflow>(&bytes).expect("step");
        let decoded = WorkflowOutput::decode(&output).expect("decode");
        assert_eq!(decoded.effects.len(), 2);
    }

    #[test]
    fn annotation_round_trip() {
        #[derive(Default)]
        struct AnnWorkflow;

        #[derive(Default, Serialize, Deserialize)]
        struct AnnState;

        #[derive(Serialize, Deserialize)]
        struct AnnEvent;

        #[derive(Serialize, Deserialize, Clone)]
        struct AnnPayload {
            message: String,
        }

        impl Workflow for AnnWorkflow {
            type State = AnnState;
            type Event = AnnEvent;
            type Ann = AnnPayload;

            fn reduce(
                &mut self,
                _event: Self::Event,
                ctx: &mut WorkflowCtx<Self::State, Self::Ann>,
            ) -> Result<(), ReduceError> {
                ctx.annotate(AnnPayload {
                    message: "hi".into(),
                });
                Ok(())
            }
        }

        let input = WorkflowInput {
            version: aos_wasm_abi::ABI_VERSION,
            state: None,
            event: DomainEvent::new("schema", serde_cbor::to_vec(&AnnEvent).unwrap()),
            ctx: Some(context_bytes("com.acme/AnnWorkflow@1")),
        };
        let bytes = input.encode().unwrap();
        let output = step_bytes::<AnnWorkflow>(&bytes).unwrap();
        let decoded = WorkflowOutput::decode(&output).unwrap();
        let ann_bytes = decoded.ann.expect("ann");
        let ann: AnnPayload = serde_cbor::from_slice(&ann_bytes).unwrap();
        assert_eq!(ann.message, "hi");
    }

    #[test]
    fn effect_receipt_envelope_decodes_payload() {
        #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
        struct DummyReceipt {
            status: i32,
        }

        let payload = serde_cbor::to_vec(&DummyReceipt { status: 200 }).unwrap();
        let envelope = EffectReceiptEnvelope {
            origin_module_id: "com.acme/Workflow@1".into(),
            origin_instance_key: Some(b"key-1".to_vec()),
            intent_id: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .into(),
            effect_kind: "http.request".into(),
            params_hash: None,
            issuer_ref: None,
            receipt_payload: payload,
            status: "ok".into(),
            emitted_at_seq: 42,
            adapter_id: "http.mock".into(),
            cost_cents: Some(0),
            signature: vec![0; 64],
        };

        let decoded: DummyReceipt = envelope.decode_receipt_payload().unwrap();
        assert_eq!(decoded, DummyReceipt { status: 200 });
    }

    #[test]
    fn emit_tracked_registers_pending_handle() {
        #[derive(Default)]
        struct TrackedWorkflow;

        #[derive(Default, Serialize, Deserialize)]
        struct TrackedState {
            pending: PendingEffects,
        }

        #[derive(Serialize, Deserialize)]
        struct TrackedEvent;

        #[derive(Serialize, Deserialize)]
        struct TrackedParams {
            prompt: String,
        }

        impl Workflow for TrackedWorkflow {
            type State = TrackedState;
            type Event = TrackedEvent;
            type Ann = Value;

            fn reduce(
                &mut self,
                _event: Self::Event,
                ctx: &mut WorkflowCtx<Self::State>,
            ) -> Result<(), ReduceError> {
                let mut pending = core::mem::take(&mut ctx.state.pending);
                let handle = ctx.effects().emit_tracked(
                    &mut pending,
                    "llm.generate",
                    &TrackedParams {
                        prompt: "hello".into(),
                    },
                    Some("llm"),
                );
                ctx.state.pending = pending;
                assert_eq!(handle.effect_kind, "llm.generate");
                Ok(())
            }
        }

        let input = WorkflowInput {
            version: aos_wasm_abi::ABI_VERSION,
            state: None,
            event: DomainEvent::new("schema", serde_cbor::to_vec(&TrackedEvent).unwrap()),
            ctx: Some(context_bytes("com.acme/TrackedWorkflow@1")),
        };
        let bytes = input.encode().unwrap();
        let output = step_bytes::<TrackedWorkflow>(&bytes).expect("step");
        let decoded = WorkflowOutput::decode(&output).expect("decode");
        assert_eq!(decoded.effects.len(), 1);
        let state: TrackedState =
            serde_cbor::from_slice(decoded.state.as_ref().expect("state")).expect("state decode");
        assert_eq!(state.pending.len(), 1);
    }

    #[test]
    fn event_union_requires_canonical_tagged_payload() {
        #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
        struct StartPayload {
            id: u64,
        }

        aos_event_union! {
            #[derive(Debug, Clone, Serialize, PartialEq, Eq)]
            enum UnionEvent {
                Start(StartPayload),
            }
        }

        let tagged = serde_cbor::to_vec(&serde_cbor::Value::Map(BTreeMap::from([
            (
                serde_cbor::Value::Text("$tag".into()),
                serde_cbor::Value::Text("Start".into()),
            ),
            (
                serde_cbor::Value::Text("$value".into()),
                serde_cbor::value::to_value(StartPayload { id: 7 }).unwrap(),
            ),
        ])))
        .unwrap();
        let decoded: UnionEvent = serde_cbor::from_slice(&tagged).unwrap();
        assert_eq!(decoded, UnionEvent::Start(StartPayload { id: 7 }));

        let untagged = serde_cbor::to_vec(&StartPayload { id: 7 }).unwrap();
        assert!(serde_cbor::from_slice::<UnionEvent>(&untagged).is_err());
    }
}
