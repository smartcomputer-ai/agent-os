#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::String;
use aos_air_exec::Value as AirValue;
use aos_wasm_sdk::{aos_reducer, ReduceError, Reducer, ReducerCtx};
use serde::{Deserialize, Serialize};

const FETCH_REQUEST_SCHEMA: &str = "demo/UpgradeFetchRequest@1";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SafeUpgradeState {
    pc: SafeUpgradePc,
    next_request_id: u64,
    pending_request: Option<u64>,
    primary_status: Option<i64>,
    follow_status: Option<i64>,
    requests_observed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum SafeUpgradePc {
    Idle,
    Fetching,
    Completed,
}

impl Default for SafeUpgradePc {
    fn default() -> Self {
        SafeUpgradePc::Idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum SafeUpgradeEvent {
    Start { url: String },
    NotifyComplete {
        primary_status: i64,
        follow_status: i64,
        request_count: u64,
    },
}

aos_reducer!(SafeUpgradeSm);

#[derive(Default)]
struct SafeUpgradeSm;

impl Reducer for SafeUpgradeSm {
    type State = SafeUpgradeState;
    type Event = SafeUpgradeEvent;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
        match event {
            SafeUpgradeEvent::Start { url } => handle_start(ctx, url),
            SafeUpgradeEvent::NotifyComplete {
                primary_status,
                follow_status,
                request_count,
            } => handle_notify(ctx, primary_status, follow_status, request_count),
        }
        Ok(())
    }
}

fn handle_start(ctx: &mut ReducerCtx<SafeUpgradeState, ()>, url: String) {
    if matches!(ctx.state.pc, SafeUpgradePc::Fetching) {
        return;
    }

    let request_id = ctx.state.next_request_id;
    ctx.state.next_request_id = ctx.state.next_request_id.saturating_add(1);
    ctx.state.pending_request = Some(request_id);
    ctx.state.pc = SafeUpgradePc::Fetching;
    ctx.state.primary_status = None;
    ctx.state.follow_status = None;
    ctx.state.requests_observed = 0;

    let intent_value = AirValue::record([
        ("request_id", AirValue::Nat(request_id)),
        ("url", AirValue::Text(url)),
    ]);
    let key = request_id.to_be_bytes();
    ctx.intent(FETCH_REQUEST_SCHEMA)
        .key_bytes(&key)
        .payload(&intent_value)
        .send();
}

fn handle_notify(
    ctx: &mut ReducerCtx<SafeUpgradeState, ()>,
    primary_status: i64,
    follow_status: i64,
    request_count: u64,
) {
    if ctx.state.pending_request.is_none() {
        return;
    }

    let follow_value = if follow_status < 0 {
        None
    } else {
        Some(follow_status)
    };
    ctx.state.pending_request = None;
    ctx.state.pc = SafeUpgradePc::Completed;
    ctx.state.primary_status = Some(primary_status);
    ctx.state.follow_status = follow_value;
    ctx.state.requests_observed = request_count;
}
