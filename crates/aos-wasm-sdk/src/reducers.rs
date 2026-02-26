use alloc::string::{String, ToString};
use alloc::vec::Vec;
use aos_wasm_abi::{
    AbiDecodeError, AbiEncodeError, DomainEvent as AbiDomainEvent, ReducerContext,
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
    context: Option<ReducerContext>,
    reducer_name: &'static str,
    domain_events: Vec<PendingEvent>,
    effects: Vec<PendingEffect>,
}

impl<S, A> ReducerCtx<S, A> {
    fn new(state: S, ctx: Option<ReducerContext>, reducer_name: &'static str) -> Self {
        Self {
            state,
            ann: None,
            context: ctx,
            reducer_name,
            domain_events: Vec::new(),
            effects: Vec::new(),
        }
    }

    /// Optional cell key provided by the kernel.
    pub fn key(&self) -> Option<&[u8]> {
        self.context.as_ref().and_then(|ctx| ctx.key.as_deref())
    }

    /// Return the key and fail deterministically if the reducer is keyed but no key was supplied.
    pub fn key_required(&self) -> Result<&[u8], ReduceError> {
        self.context
            .as_ref()
            .and_then(|ctx| ctx.key.as_deref())
            .ok_or(ReduceError::new("missing reducer key"))
    }

    /// Return the key interpreted as UTF-8 text (fails if absent or invalid UTF-8).
    pub fn key_text(&self) -> Result<&str, ReduceError> {
        let bytes = self.key_required()?;
        core::str::from_utf8(bytes).map_err(|_| ReduceError::new("key not utf8"))
    }

    /// Enforce that the provided bytes exactly match the reducer key (when present).
    pub fn ensure_key_eq(&self, expected: &[u8]) -> Result<(), ReduceError> {
        match self.context.as_ref().and_then(|ctx| ctx.key.as_deref()) {
            Some(actual) if actual == expected => Ok(()),
            Some(_) => Err(ReduceError::new("key mismatch")),
            None => Err(ReduceError::new("missing reducer key")),
        }
    }

    /// Name of the reducer type (for diagnostics).
    pub fn reducer_name(&self) -> &'static str {
        self.reducer_name
    }

    /// Access the full call context supplied by the kernel.
    pub fn context(&self) -> Option<&ReducerContext> {
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

    /// Reducer module name string.
    pub fn reducer_module(&self) -> Option<&str> {
        self.context.as_ref().map(|ctx| ctx.reducer.as_str())
    }

    /// Whether the reducer is operating in keyed mode.
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

    fn emit_effect(&mut self, effect: PendingEffect) {
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
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs: Option<Vec<HashRef>>,
}

/// Blob.get parameters.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BlobGetParams {
    pub blob_ref: HashRef,
}

/// Content-addressed blob reference.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct HashRef {
    pub algorithm: String,
    #[serde(with = "serde_bytes")]
    pub digest: Vec<u8>,
}

/// Generic workflow receipt envelope delivered to reducers for external effects.
///
/// The `receipt_payload` field contains the canonical CBOR payload validated against
/// the effect's declared `receipt_schema` in the kernel.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct EffectReceiptEnvelope {
    pub origin_module_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub origin_instance_key: Option<Vec<u8>>,
    pub intent_id: String,
    pub effect_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_hash: Option<String>,
    #[serde(with = "serde_bytes")]
    pub receipt_payload: Vec<u8>,
    pub status: String,
    pub emitted_at_seq: u64,
    pub adapter_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cents: Option<u64>,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
}

impl EffectReceiptEnvelope {
    /// Decode the embedded receipt payload into a typed struct.
    pub fn decode_receipt_payload<T: DeserializeOwned>(&self) -> Result<T, serde_cbor::Error> {
        serde_cbor::from_slice(&self.receipt_payload)
    }
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

mod serde_bytes_opt {
    use alloc::vec::Vec;
    use serde::{Deserialize, Deserializer, Serializer};
    use serde_bytes::{ByteBuf, Bytes};

    pub fn serialize<S>(value: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(bytes) => serializer.serialize_some(Bytes::new(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<ByteBuf>::deserialize(deserializer).map(|opt| opt.map(|buf| buf.into_vec()))
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
    let context = match &input.ctx {
        Some(bytes) => Some(ReducerContext::decode(bytes).map_err(StepError::CtxDecode)?),
        None => None,
    };
    let mut ctx = ReducerCtx::new(state, context, reducer_name);
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
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use aos_wasm_abi::{DomainEvent, ReducerContext, ReducerInput};
    use serde::{Deserialize, Serialize};

    fn context_bytes(reducer: &str) -> Vec<u8> {
        let ctx = ReducerContext {
            now_ns: 1,
            logical_now_ns: 2,
            journal_height: 3,
            entropy: vec![0x11; 64],
            event_hash: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
            manifest_hash:
                "sha256:1111111111111111111111111111111111111111111111111111111111111111".into(),
            reducer: reducer.into(),
            key: None,
            cell_mode: false,
        };
        serde_cbor::to_vec(&ctx).expect("context bytes")
    }

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
        let input = ReducerInput {
            version: aos_wasm_abi::ABI_VERSION,
            state: None,
            event: DomainEvent::new(
                "schema",
                serde_cbor::to_vec(&TestEvent::Increment(1)).unwrap(),
            ),
            ctx: Some(context_bytes("com.acme/TestReducer@1")),
        };
        let bytes = input.encode().unwrap();
        let output = step_bytes::<TestReducer>(&bytes).expect("step");
        let decoded = ReducerOutput::decode(&output).expect("decode");
        assert_eq!(decoded.domain_events.len(), 1);
        let payload: u64 = serde_cbor::from_slice(&decoded.domain_events[0].value).unwrap();
        assert_eq!(payload, 1);
    }

    #[test]
    fn reducer_can_emit_multiple_effects() {
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
            ctx: Some(context_bytes("com.acme/EffectReducer@1")),
        };
        let bytes = input.encode().unwrap();
        let output = step_bytes::<EffectReducer>(&bytes).expect("step");
        let decoded = ReducerOutput::decode(&output).expect("decode");
        assert_eq!(decoded.effects.len(), 2);
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
            ctx: Some(context_bytes("com.acme/AnnReducer@1")),
        };
        let bytes = input.encode().unwrap();
        let output = step_bytes::<AnnReducer>(&bytes).unwrap();
        let decoded = ReducerOutput::decode(&output).unwrap();
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
