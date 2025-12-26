use alloc::string::{String, ToString};
use alloc::vec::Vec;
use aos_wasm_abi::{
    AbiDecodeError, AbiEncodeError, CallContext, DomainEvent as AbiDomainEvent,
    ReducerEffect as AbiReducerEffect, ReducerInput, ReducerOutput,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{read_input, write_back};
use serde_cbor::Value;

/// Trait implemented by every reducer.
pub trait Reducer: Default {
    /// Durable reducer state; persisted via canonical CBOR.
    type State: Serialize + DeserializeOwned + Default;

    /// Event family consumed by this reducer.
    type Event: DeserializeOwned;

    /// Optional annotation payload for observability.
    type Ann: Serialize;

    /// Core reducer logic.
    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError>;
}

/// Reducer execution context passed to `Reducer::reduce`.
pub struct ReducerCtx<S, A = Value> {
    pub state: S,
    ann: Option<A>,
    key: Option<Vec<u8>>,
    reducer_name: &'static str,
    domain_events: Vec<PendingEvent>,
    effects: Vec<PendingEffect>,
    effect_used: bool,
}

impl<S, A> ReducerCtx<S, A> {
    fn new(state: S, ctx: CallContext, reducer_name: &'static str) -> Self {
        Self {
            state,
            ann: None,
            key: ctx.key,
            reducer_name,
            domain_events: Vec::new(),
            effects: Vec::new(),
            effect_used: false,
        }
    }

    /// Optional cell key provided by the kernel.
    pub fn key(&self) -> Option<&[u8]> {
        self.key.as_deref()
    }

    /// Return the key and fail deterministically if the reducer is keyed but no key was supplied.
    pub fn key_required(&self) -> Result<&[u8], ReduceError> {
        self.key
            .as_deref()
            .ok_or(ReduceError::new("missing reducer key"))
    }

    /// Return the key interpreted as UTF-8 text (fails if absent or invalid UTF-8).
    pub fn key_text(&self) -> Result<&str, ReduceError> {
        let bytes = self.key_required()?;
        core::str::from_utf8(bytes).map_err(|_| ReduceError::new("key not utf8"))
    }

    /// Enforce that the provided bytes exactly match the reducer key (when present).
    pub fn ensure_key_eq(&self, expected: &[u8]) -> Result<(), ReduceError> {
        match &self.key {
            Some(actual) if actual.as_slice() == expected => Ok(()),
            Some(_) => Err(ReduceError::new("key mismatch")),
            None => Err(ReduceError::new("missing reducer key")),
        }
    }

    /// Name of the reducer type (for diagnostics).
    pub fn reducer_name(&self) -> &'static str {
        self.reducer_name
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

    /// Emit micro-effects (at most one per reduce call).
    pub fn effects(&mut self) -> Effects<'_, S, A> {
        Effects { ctx: self }
    }

    fn push_event(&mut self, event: PendingEvent) {
        self.domain_events.push(event);
    }

    fn emit_effect(&mut self, effect: PendingEffect) {
        if self.effect_used {
            self.trap(StepError::TooManyEffects);
        }
        self.effect_used = true;
        self.effects.push(effect);
    }

    fn finish(self) -> Result<ReducerOutput, StepError>
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
        Ok(ReducerOutput {
            state: Some(state_bytes),
            domain_events,
            effects,
            ann: ann_bytes,
        })
    }

    #[track_caller]
    fn trap(&self, err: StepError) -> ! {
        err.trap(self.reducer_name)
    }
}

/// Domain intent builder.
pub struct IntentBuilder<'ctx, S, A> {
    ctx: &'ctx mut ReducerCtx<S, A>,
    schema: &'static str,
    key: Option<Vec<u8>>,
    payload: IntentPayload,
}

impl<'ctx, S, A> IntentBuilder<'ctx, S, A> {
    fn new(ctx: &'ctx mut ReducerCtx<S, A>, schema: &'static str) -> Self {
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
    ctx: &'ctx mut ReducerCtx<S, A>,
}

impl<'ctx, S, A> Effects<'ctx, S, A> {
    /// Emit a timer.set micro-effect.
    pub fn timer_set(&mut self, params: &TimerSetParams, cap_slot: &str) {
        self.emit_raw(EFFECT_TIMER_SET, params, Some(cap_slot));
    }

    /// Emit a blob.put micro-effect.
    pub fn blob_put(&mut self, params: &BlobPutParams, cap_slot: &str) {
        self.emit_raw(EFFECT_BLOB_PUT, params, Some(cap_slot));
    }

    /// Emit a blob.get micro-effect.
    pub fn blob_get(&mut self, params: &BlobGetParams, cap_slot: &str) {
        self.emit_raw(EFFECT_BLOB_GET, params, Some(cap_slot));
    }

    /// Escape hatch for future micro-effects.
    pub fn emit_raw(
        &mut self,
        kind: &'static str,
        params: &impl Serialize,
        cap_slot: Option<&str>,
    ) {
        self.emit_raw_with_idempotency(kind, params, cap_slot, None);
    }

    /// Emit a micro-effect with an explicit idempotency key (32 bytes).
    pub fn emit_raw_with_idempotency(
        &mut self,
        kind: &'static str,
        params: &impl Serialize,
        cap_slot: Option<&str>,
        idempotency_key: Option<&[u8]>,
    ) {
        let payload = match serde_cbor::to_vec(params) {
            Ok(bytes) => bytes,
            Err(err) => self.ctx.trap(StepError::EffectPayload(err)),
        };
        let key = idempotency_key.map(|bytes| bytes.to_vec());
        self.ctx.emit_effect(PendingEffect {
            kind,
            params: payload,
            cap_slot: cap_slot.map(|s| s.to_string()),
            idempotency_key: key,
        });
    }
}

/// Timer.set parameters (canonical schema subset).
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TimerSetParams {
    pub deliver_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Blob.put parameters.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BlobPutParams {
    pub namespace: String,
    pub blob_ref: HashRef,
}

/// Blob.get parameters.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BlobGetParams {
    pub namespace: String,
    pub key: String,
}

/// Content-addressed blob reference.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct HashRef {
    pub algorithm: String,
    #[serde(with = "serde_bytes")]
    pub digest: Vec<u8>,
}

const EFFECT_TIMER_SET: &str = "timer.set";
const EFFECT_BLOB_PUT: &str = "blob.put";
const EFFECT_BLOB_GET: &str = "blob.get";

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

struct PendingEffect {
    kind: &'static str,
    params: Vec<u8>,
    cap_slot: Option<String>,
    idempotency_key: Option<Vec<u8>>,
}

impl PendingEffect {
    fn into_abi(self) -> AbiReducerEffect {
        let mut eff = AbiReducerEffect::new(self.kind, self.params);
        if let Some(slot) = self.cap_slot {
            eff.cap_slot = Some(slot);
        }
        if let Some(key) = self.idempotency_key {
            eff.idempotency_key = Some(key);
        }
        eff
    }
}

/// Deterministic reducer failure.
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

/// Errors surfaced while running the reducer entrypoint.
#[derive(Debug)]
pub enum StepError {
    AbiDecode(AbiDecodeError),
    StateDecode(serde_cbor::Error),
    EventDecode(serde_cbor::Error),
    StateEncode(serde_cbor::Error),
    AnnEncode(serde_cbor::Error),
    OutputEncode(AbiEncodeError),
    IntentPayload(serde_cbor::Error),
    EffectPayload(serde_cbor::Error),
    TooManyEffects,
    Reduce(&'static str),
}

impl StepError {
    #[cold]
    pub fn trap(self, reducer: &'static str) -> ! {
        panic!("aos-wasm-sdk({reducer}): {self}");
    }
}

impl core::fmt::Display for StepError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StepError::AbiDecode(err) => write!(f, "abi decode failed: {err}"),
            StepError::StateDecode(err) => write!(f, "state decode failed: {err}"),
            StepError::EventDecode(err) => write!(f, "event decode failed: {err}"),
            StepError::StateEncode(err) => write!(f, "state encode failed: {err}"),
            StepError::AnnEncode(err) => write!(f, "annotation encode failed: {err}"),
            StepError::OutputEncode(err) => write!(f, "output encode failed: {err}"),
            StepError::IntentPayload(err) => write!(f, "intent payload encode failed: {err}"),
            StepError::EffectPayload(err) => write!(f, "effect payload encode failed: {err}"),
            StepError::TooManyEffects => write!(f, "reducers may emit at most one micro-effect"),
            StepError::Reduce(msg) => write!(f, "reduce error: {msg}"),
        }
    }
}

/// Execute a reducer against a reducer input envelope (used by tests).
pub fn step_bytes<R: Reducer>(input: &[u8]) -> Result<Vec<u8>, StepError>
where
    R::State: Serialize,
    R::Ann: Serialize,
{
    let env = ReducerInput::decode(input).map_err(StepError::AbiDecode)?;
    run_reducer::<R>(env)
}

fn run_reducer<R: Reducer>(input: ReducerInput) -> Result<Vec<u8>, StepError>
where
    R::State: Serialize,
    R::Ann: Serialize,
{
    let reducer_name = core::any::type_name::<R>();
    let state = match input.state {
        Some(bytes) => serde_cbor::from_slice(&bytes).map_err(StepError::StateDecode)?,
        None => R::State::default(),
    };
    let event = serde_cbor::from_slice(&input.event.value).map_err(StepError::EventDecode)?;
    let mut ctx = ReducerCtx::new(state, input.ctx, reducer_name);
    let mut reducer = R::default();
    reducer
        .reduce(event, &mut ctx)
        .map_err(|err| StepError::Reduce(err.message()))?;
    let output = ctx.finish()?;
    output.encode().map_err(StepError::OutputEncode)
}

/// Entry helper for exported `step`.
pub fn dispatch_reducer<R: Reducer>(ptr: i32, len: i32) -> (i32, i32)
where
    R::State: Serialize,
    R::Ann: Serialize,
{
    let reducer_name = core::any::type_name::<R>();
    let bytes = unsafe { read_input(ptr, len) };
    match step_bytes::<R>(bytes) {
        Ok(output) => write_back(&output),
        Err(err) => err.trap(reducer_name),
    }
}

/// Macro wiring the reducer entrypoints.
#[macro_export]
macro_rules! aos_reducer {
    ($ty:ty) => {
        #[cfg_attr(target_arch = "wasm32", unsafe(export_name = "alloc"))]
        pub extern "C" fn aos_wasm_alloc(len: i32) -> i32 {
            $crate::exported_alloc(len)
        }

        #[cfg_attr(target_arch = "wasm32", unsafe(export_name = "step"))]
        pub extern "C" fn aos_wasm_step(ptr: i32, len: i32) -> (i32, i32) {
            $crate::dispatch_reducer::<$ty>(ptr, len)
        }
    };
}

/// Helper macro to apply AIR's canonical variant tagging (`$tag`/`$value`) to an enum.
///
/// This avoids repeating `#[serde(tag = "$tag", content = "$value")]` on every reducer
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

/// Helper macro to define an enum that can deserialize either a tagged (canonical `$tag`/`$value`)
/// variant form or any number of plain record forms (useful for mixing app events with receipt records).
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
                $(
                    // First, try canonical tagged form: {"$tag":"Variant", "$value": ...}
                    if let serde_cbor::Value::Map(map) = &value {
                        if let Some(serde_cbor::Value::Text(tag)) = map.get(&serde_cbor::Value::Text("$tag".into())) {
                            let expected = stringify!($variant);
                            let expected_lower = expected.to_ascii_lowercase();
                            if tag == expected || tag == expected_lower.as_str() {
                                let inner = map.get(&serde_cbor::Value::Text("$value".into()))
                                    .cloned()
                                    .unwrap_or(serde_cbor::Value::Null);
                                if let Ok(v) = serde_cbor::value::from_value::<$ty>(inner) {
                                    return Ok(Self::$variant(v));
                                }
                            }
                        }
                    }
                    // Fallback: try to deserialize the entire value as the variant payload (record-shaped receipts, legacy untagged Start, etc.)
                    if let Ok(v) = serde_cbor::value::from_value::<$ty>(value.clone()) {
                        return Ok(Self::$variant(v));
                    }
                )+
                Err(serde::de::Error::custom("no union variant matched payload"))
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use aos_wasm_abi::{CallContext, DomainEvent, ReducerInput};
    use serde::{Deserialize, Serialize};

    #[derive(Default)]
    struct TestReducer;

    #[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
    struct TestState {
        counter: u64,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum TestEvent {
        Increment(u64),
    }

    impl Reducer for TestReducer {
        type State = TestState;
        type Event = TestEvent;
        type Ann = Value;

        fn reduce(
            &mut self,
            event: Self::Event,
            ctx: &mut ReducerCtx<Self::State>,
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
        let ctx = CallContext::new(false, None);
        let input = ReducerInput {
            version: aos_wasm_abi::ABI_VERSION,
            state: None,
            event: DomainEvent::new(
                "schema",
                serde_cbor::to_vec(&TestEvent::Increment(1)).unwrap(),
            ),
            ctx,
        };
        let bytes = input.encode().unwrap();
        let output = step_bytes::<TestReducer>(&bytes).expect("step");
        let decoded = ReducerOutput::decode(&output).expect("decode");
        assert_eq!(decoded.domain_events.len(), 1);
        let payload: u64 = serde_cbor::from_slice(&decoded.domain_events[0].value).unwrap();
        assert_eq!(payload, 1);
    }

    #[test]
    #[should_panic(expected = "aos-wasm-sdk(")]
    fn effects_guard_enforces_single_effect() {
        #[derive(Default)]
        struct EffectReducer;

        #[derive(Default, Serialize, Deserialize)]
        struct EffectState;

        #[derive(Serialize, Deserialize)]
        struct EffectEvent;

        impl Reducer for EffectReducer {
            type State = EffectState;
            type Event = EffectEvent;
            type Ann = Value;

            fn reduce(
                &mut self,
                _event: Self::Event,
                ctx: &mut ReducerCtx<Self::State>,
            ) -> Result<(), ReduceError> {
                let params = TimerSetParams {
                    deliver_at_ns: 42,
                    key: None,
                };
                ctx.effects().timer_set(&params, "clock");
                ctx.effects().timer_set(&params, "clock");
                Ok(())
            }
        }

        let input = ReducerInput {
            version: aos_wasm_abi::ABI_VERSION,
            state: None,
            event: DomainEvent::new("schema", serde_cbor::to_vec(&EffectEvent).unwrap()),
            ctx: CallContext::new(false, None),
        };
        let bytes = input.encode().unwrap();
        let _ = step_bytes::<EffectReducer>(&bytes);
    }

    #[test]
    fn annotation_round_trip() {
        #[derive(Default)]
        struct AnnReducer;

        #[derive(Default, Serialize, Deserialize)]
        struct AnnState;

        #[derive(Serialize, Deserialize)]
        struct AnnEvent;

        #[derive(Serialize, Deserialize, Clone)]
        struct AnnPayload {
            message: String,
        }

        impl Reducer for AnnReducer {
            type State = AnnState;
            type Event = AnnEvent;
            type Ann = AnnPayload;

            fn reduce(
                &mut self,
                _event: Self::Event,
                ctx: &mut ReducerCtx<Self::State, Self::Ann>,
            ) -> Result<(), ReduceError> {
                ctx.annotate(AnnPayload {
                    message: "hi".into(),
                });
                Ok(())
            }
        }

        let input = ReducerInput {
            version: aos_wasm_abi::ABI_VERSION,
            state: None,
            event: DomainEvent::new("schema", serde_cbor::to_vec(&AnnEvent).unwrap()),
            ctx: CallContext::new(false, None),
        };
        let bytes = input.encode().unwrap();
        let output = step_bytes::<AnnReducer>(&bytes).unwrap();
        let decoded = ReducerOutput::decode(&output).unwrap();
        let ann_bytes = decoded.ann.expect("ann");
        let ann: AnnPayload = serde_cbor::from_slice(&ann_bytes).unwrap();
        assert_eq!(ann.message, "hi");
    }
}
