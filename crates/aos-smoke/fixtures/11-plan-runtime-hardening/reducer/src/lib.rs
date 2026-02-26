#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::{collections::BTreeMap, string::String, vec::Vec};
use aos_wasm_sdk::{
    EffectReceiptEnvelope, ReduceError, Reducer, ReducerCtx, aos_reducer, aos_variant,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const HTTP_REQUEST_EFFECT: &str = "http.request";

aos_reducer!(FlowTracker);

#[derive(Default)]
struct FlowTracker;

impl Reducer for FlowTracker {
    type State = FlowState;
    type Event = RuntimeHardeningEvent;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
        match event {
            RuntimeHardeningEvent::Start(start) => handle_start(ctx, start),
            RuntimeHardeningEvent::Approval(approval) => handle_approval(ctx, approval)?,
            RuntimeHardeningEvent::Receipt(envelope) => handle_receipt(ctx, envelope)?,
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FlowState {
    completed_count: u64,
    last_request_id: Option<u64>,
    last_worker_count: Option<u64>,
    requests: Vec<FlowRequest>,
    pending_by_hash: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FlowRequest {
    request_id: u64,
    approved: bool,
    worker_count: u64,
    completed_workers: u64,
    urls: Vec<String>,
    done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RuntimeHardeningStart {
    request_id: u64,
    urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApprovalEvent {
    request_id: u64,
    approved: bool,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum RuntimeHardeningEvent {
        Start(RuntimeHardeningStart),
        Approval(ApprovalEvent),
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

fn handle_start(ctx: &mut ReducerCtx<FlowState, ()>, start: RuntimeHardeningStart) {
    ctx.state
        .pending_by_hash
        .retain(|_, request_id| *request_id != start.request_id);

    let request = FlowRequest {
        request_id: start.request_id,
        approved: false,
        worker_count: start.urls.len() as u64,
        completed_workers: 0,
        urls: start.urls,
        done: false,
    };

    if let Some(idx) = find_request_index(&ctx.state.requests, request.request_id) {
        ctx.state.requests[idx] = request;
    } else {
        ctx.state.requests.push(request);
    }
}

fn handle_approval(
    ctx: &mut ReducerCtx<FlowState, ()>,
    approval: ApprovalEvent,
) -> Result<(), ReduceError> {
    if !approval.approved {
        return Ok(());
    }

    let Some(idx) = find_request_index(&ctx.state.requests, approval.request_id) else {
        return Ok(());
    };

    if ctx.state.requests[idx].done || ctx.state.requests[idx].approved {
        return Ok(());
    }

    ctx.state.requests[idx].approved = true;
    let worker_count = ctx.state.requests[idx].worker_count;
    if worker_count == 0 {
        finalize_request(ctx, approval.request_id, 0);
        return Ok(());
    }

    let urls = ctx.state.requests[idx].urls.clone();
    for url in urls {
        let params = HttpRequestParams {
            method: "GET".into(),
            url,
            headers: BTreeMap::new(),
            body_ref: None,
        };
        let params_hash = hash_request_params(&params)?;
        ctx.state
            .pending_by_hash
            .insert(params_hash, approval.request_id);
        ctx.effects()
            .emit_raw(HTTP_REQUEST_EFFECT, &params, Some("default"));
    }

    Ok(())
}

fn handle_receipt(
    ctx: &mut ReducerCtx<FlowState, ()>,
    envelope: EffectReceiptEnvelope,
) -> Result<(), ReduceError> {
    if envelope.effect_kind != HTTP_REQUEST_EFFECT {
        return Ok(());
    }

    let Some(params_hash) = envelope.params_hash.as_ref() else {
        return Err(ReduceError::new("missing params_hash on receipt"));
    };

    let Some(request_id) = ctx.state.pending_by_hash.remove(params_hash.as_str()) else {
        return Ok(());
    };

    let Some(idx) = find_request_index(&ctx.state.requests, request_id) else {
        return Err(ReduceError::new("receipt matched unknown request"));
    };

    if ctx.state.requests[idx].done {
        return Ok(());
    }

    ctx.state.requests[idx].completed_workers = ctx.state.requests[idx]
        .completed_workers
        .saturating_add(1);

    if ctx.state.requests[idx].completed_workers >= ctx.state.requests[idx].worker_count {
        let worker_count = ctx.state.requests[idx].worker_count;
        ctx.state.requests[idx].done = true;
        finalize_request(ctx, request_id, worker_count);
    }

    Ok(())
}

fn finalize_request(ctx: &mut ReducerCtx<FlowState, ()>, request_id: u64, worker_count: u64) {
    ctx.state
        .pending_by_hash
        .retain(|_, pending_request_id| *pending_request_id != request_id);
    ctx.state.completed_count = ctx.state.completed_count.saturating_add(1);
    ctx.state.last_request_id = Some(request_id);
    ctx.state.last_worker_count = Some(worker_count);
}

fn find_request_index(requests: &[FlowRequest], request_id: u64) -> Option<usize> {
    requests.iter().position(|request| request.request_id == request_id)
}

fn hash_request_params(params: &HttpRequestParams) -> Result<String, ReduceError> {
    let bytes =
        serde_cbor::to_vec(params).map_err(|_| ReduceError::new("failed to encode http params"))?;
    let digest = Sha256::digest(&bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push(nibble_to_hex((byte >> 4) & 0x0f));
        hex.push(nibble_to_hex(byte & 0x0f));
    }
    Ok(alloc::format!("sha256:{hex}"))
}

fn nibble_to_hex(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'a' + (nibble - 10)) as char,
    }
}
