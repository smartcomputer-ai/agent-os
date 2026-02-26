#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::{collections::BTreeMap, format, string::String, vec::Vec};
use aos_wasm_sdk::{
    EffectReceiptEnvelope, ReduceError, Reducer, ReducerCtx, aos_reducer, aos_variant,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const HTTP_REQUEST_EFFECT: &str = "http.request";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AggregatorState {
    pc: AggregatorPc,
    next_request_id: u64,
    pending_request: Option<u64>,
    current_topic: Option<String>,
    pending_targets: Vec<String>,
    pending_by_hash: BTreeMap<String, String>,
    last_responses: Vec<AggregateResponse>,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum AggregatorPc {
        Idle,
        Running,
        Done,
    }
}

impl Default for AggregatorPc {
    fn default() -> Self {
        AggregatorPc::Idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AggregationTarget {
    name: String,
    url: String,
    method: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AggregateResponse {
    source: String,
    status: i64,
    body_ref: Option<String>,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum AggregatorEvent {
    Start {
        topic: String,
        primary: AggregationTarget,
        secondary: AggregationTarget,
        tertiary: AggregationTarget,
    },
    Receipt(EffectReceiptEnvelope),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HttpRequestParams {
    method: String,
    url: String,
    headers: BTreeMap<String, String>,
    body_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RequestTimings {
    start_ns: u64,
    end_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HttpRequestReceipt {
    status: i32,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    body_ref: Option<String>,
    timings: RequestTimings,
    adapter_id: String,
}

aos_reducer!(AggregatorSm);

#[derive(Default)]
struct AggregatorSm;

impl Reducer for AggregatorSm {
    type State = AggregatorState;
    type Event = AggregatorEvent;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
        match event {
            AggregatorEvent::Start {
                topic,
                primary,
                secondary,
                tertiary,
            } => handle_start(ctx, topic, primary, secondary, tertiary)?,
            AggregatorEvent::Receipt(envelope) => handle_receipt(ctx, envelope)?,
        }
        Ok(())
    }
}

fn handle_start(
    ctx: &mut ReducerCtx<AggregatorState, ()>,
    topic: String,
    primary: AggregationTarget,
    secondary: AggregationTarget,
    tertiary: AggregationTarget,
) -> Result<(), ReduceError> {
    if matches!(ctx.state.pc, AggregatorPc::Running) {
        return Ok(());
    }
    let request_id = ctx.state.next_request_id;
    ctx.state.next_request_id = ctx.state.next_request_id.saturating_add(1);
    ctx.state.pending_request = Some(request_id);
    ctx.state.current_topic = Some(topic.clone());
    ctx.state.pc = AggregatorPc::Running;
    ctx.state.pending_targets.clear();
    ctx.state.pending_by_hash.clear();
    ctx.state.last_responses.clear();

    queue_http_request(ctx, primary)?;
    queue_http_request(ctx, secondary)?;
    queue_http_request(ctx, tertiary)?;

    Ok(())
}

fn queue_http_request(
    ctx: &mut ReducerCtx<AggregatorState, ()>,
    target: AggregationTarget,
) -> Result<(), ReduceError> {
    let params = HttpRequestParams {
        method: target.method,
        url: target.url,
        headers: BTreeMap::new(),
        body_ref: None,
    };
    let params_hash = hash_request_params(&params)?;
    ctx.state.pending_targets.push(target.name.clone());
    ctx.state.pending_by_hash.insert(params_hash, target.name);
    ctx.effects()
        .emit_raw(HTTP_REQUEST_EFFECT, &params, Some("default"));
    Ok(())
}

fn handle_receipt(
    ctx: &mut ReducerCtx<AggregatorState, ()>,
    envelope: EffectReceiptEnvelope,
) -> Result<(), ReduceError> {
    if ctx.state.pending_request.is_none() {
        return Ok(());
    }
    if envelope.effect_kind != HTTP_REQUEST_EFFECT {
        return Ok(());
    }
    let Some(params_hash) = envelope.params_hash.as_ref() else {
        return Err(ReduceError::new("missing params_hash on receipt"));
    };
    let Some(source) = ctx.state.pending_by_hash.remove(params_hash.as_str()) else {
        // Ignore duplicates or stale receipts for already-settled requests.
        return Ok(());
    };
    let receipt: HttpRequestReceipt = envelope
        .decode_receipt_payload()
        .map_err(|_| ReduceError::new("invalid http.request receipt payload"))?;

    ctx.state
        .pending_targets
        .retain(|name| name != source.as_str());
    ctx.state.last_responses.push(AggregateResponse {
        source,
        status: receipt.status as i64,
        body_ref: receipt.body_ref,
    });
    ctx.state
        .last_responses
        .sort_by(|left, right| left.source.cmp(&right.source));

    if ctx.state.pending_by_hash.is_empty() {
        ctx.state.pending_request = None;
        ctx.state.pc = AggregatorPc::Done;
    }

    Ok(())
}

fn hash_request_params(params: &HttpRequestParams) -> Result<String, ReduceError> {
    let bytes = serde_cbor::to_vec(params)
        .map_err(|_| ReduceError::new("failed to encode http.request params"))?;
    let digest = Sha256::digest(&bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push(nibble_to_hex((byte >> 4) & 0x0f));
        hex.push(nibble_to_hex(byte & 0x0f));
    }
    Ok(format!("sha256:{hex}"))
}

fn nibble_to_hex(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'a' + (nibble - 10)) as char,
    }
}
