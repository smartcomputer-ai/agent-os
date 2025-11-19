use std::path::Path;

use anyhow::{Context, Result, anyhow};
use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_kernel::journal::fs::FsJournal;
use aos_kernel::{Kernel, LoadedManifest};
use aos_store::{FsStore, Store};
use serde::{Deserialize, Serialize};
use serde_cbor;

use crate::support::http_harness::{HttpHarness, MockHttpResponse};
use crate::support::manifest_loader;
use crate::support::util;

const REDUCER_NAME: &str = "demo/ChainComp@1";
const EVENT_SCHEMA: &str = "demo/ChainEvent@1";
const MODULE_PATH: &str = "examples/05-chain-comp/reducer";

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ChainEventEnvelope {
    Start {
        order_id: String,
        customer_id: String,
        amount_cents: u64,
        reserve_sku: String,
        charge: ChainTargetEnvelope,
        reserve: ChainTargetEnvelope,
        notify: ChainTargetEnvelope,
        refund: ChainTargetEnvelope,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChainTargetEnvelope {
    name: String,
    method: String,
    url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChainStateView {
    phase: ChainPhaseView,
    next_request_id: u64,
    current_saga: Option<ChainSagaView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChainSagaView {
    request_id: u64,
    order_id: String,
    reserve_sku: String,
    charge_status: Option<i64>,
    reserve_status: Option<i64>,
    notify_status: Option<i64>,
    refund_status: Option<i64>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
enum ChainPhaseView {
    Idle,
    Charging,
    Reserving,
    Notifying,
    Refunding,
    Completed,
    Refunded,
}

pub fn run(example_root: &Path) -> Result<()> {
    util::reset_journal(example_root)?;
    let wasm_bytes = util::compile_reducer(MODULE_PATH)?;
    let store = std::sync::Arc::new(FsStore::open(example_root).context("open FsStore")?);
    let wasm_hash = store
        .put_blob(&wasm_bytes)
        .context("store reducer wasm blob")?;
    let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).context("hash reducer wasm")?;

    let mut loaded = manifest_loader::load_from_assets(store.clone(), example_root)?
        .ok_or_else(|| anyhow!("example 05 must provide AIR JSON assets"))?;
    patch_module_hash(&mut loaded, &wasm_hash_ref)?;

    let journal = Box::new(FsJournal::open(example_root)?);
    let kernel_config = util::kernel_config(example_root)?;
    let mut kernel = Kernel::from_loaded_manifest_with_config(
        store.clone(),
        loaded,
        journal,
        kernel_config.clone(),
    )?;

    println!("→ Chain + Compensation demo");
    submit_start(
        &mut kernel,
        ChainEventEnvelope::Start {
            order_id: "ORDER-1".into(),
            customer_id: "cust-123".into(),
            amount_cents: 1999,
            reserve_sku: "sku-123".into(),
            charge: ChainTargetEnvelope {
                name: "charge".into(),
                method: "POST".into(),
                url: "https://example.com/charge".into(),
            },
            reserve: ChainTargetEnvelope {
                name: "reserve".into(),
                method: "POST".into(),
                url: "https://example.com/reserve".into(),
            },
            notify: ChainTargetEnvelope {
                name: "notify".into(),
                method: "POST".into(),
                url: "https://example.com/notify".into(),
            },
            refund: ChainTargetEnvelope {
                name: "refund".into(),
                method: "POST".into(),
                url: "https://example.com/refund".into(),
            },
        },
    )?;

    let mut harness = HttpHarness::new();

    let mut requests = harness.collect_requests(&mut kernel)?;
    if requests.len() != 1 {
        return Err(anyhow!(
            "expected 1 charge intent, found {}",
            requests.len()
        ));
    }
    let charge_ctx = requests.remove(0);
    println!("     responding to charge");
    harness.respond_with(
        &mut kernel,
        charge_ctx,
        MockHttpResponse::json(201, "{\"charge\":\"ok\"}"),
    )?;

    let mut requests = harness.collect_requests(&mut kernel)?;
    if requests.len() != 1 {
        return Err(anyhow!(
            "expected 1 reserve intent after charge, found {}",
            requests.len()
        ));
    }
    let reserve_ctx = requests.remove(0);
    println!("     forcing reserve failure to trigger compensation");
    harness.respond_with(
        &mut kernel,
        reserve_ctx,
        MockHttpResponse::json(503, "{\"reserve\":\"error\"}"),
    )?;

    let mut requests = harness.collect_requests(&mut kernel)?;
    if requests.len() != 1 {
        return Err(anyhow!(
            "expected refund intent after failure, found {}",
            requests.len()
        ));
    }
    let refund_ctx = requests.remove(0);
    println!("     refunding original charge");
    harness.respond_with(
        &mut kernel,
        refund_ctx,
        MockHttpResponse::json(202, "{\"refund\":\"ok\"}"),
    )?;

    let final_bytes = kernel
        .reducer_state(REDUCER_NAME)
        .cloned()
        .ok_or_else(|| anyhow!("missing reducer state"))?;
    let state: ChainStateView = serde_cbor::from_slice(&final_bytes)?;
    match &state.current_saga {
        Some(saga) => {
            println!(
                "     saga request={} phase={:?} reserve={:?} refund={:?}",
                saga.request_id, state.phase, saga.reserve_status, saga.refund_status
            );
            if state.phase != ChainPhaseView::Refunded {
                return Err(anyhow!("expected refunded phase, got {:?}", state.phase));
            }
            if saga.refund_status.is_none() {
                return Err(anyhow!("refund status missing in reducer state"));
            }
        }
        None => return Err(anyhow!("expected active saga")),
    }

    drop(kernel);

    let mut replay_loaded = manifest_loader::load_from_assets(store.clone(), example_root)?
        .ok_or_else(|| anyhow!("example 05 must provide AIR JSON assets"))?;
    patch_module_hash(&mut replay_loaded, &wasm_hash_ref)?;
    let replay_journal = Box::new(FsJournal::open(example_root)?);
    let mut replay = Kernel::from_loaded_manifest_with_config(
        store.clone(),
        replay_loaded,
        replay_journal,
        kernel_config,
    )?;
    replay.tick_until_idle()?;
    let replay_bytes = replay
        .reducer_state(REDUCER_NAME)
        .cloned()
        .ok_or_else(|| anyhow!("missing replay state"))?;
    if replay_bytes != final_bytes {
        return Err(anyhow!("replay mismatch: reducer state diverged"));
    }
    let state_hash = Hash::of_bytes(&final_bytes).to_hex();
    println!("   replay check: OK (state hash {state_hash})\n");

    Ok(())
}

fn submit_start(kernel: &mut Kernel<FsStore>, event: ChainEventEnvelope) -> Result<()> {
    let ChainEventEnvelope::Start { order_id, .. } = &event;
    println!("     saga start → order_id={order_id}");
    let payload = serde_cbor::to_vec(&event)?;
    kernel.submit_domain_event(EVENT_SCHEMA, payload);
    kernel.tick_until_idle()?;
    Ok(())
}

fn patch_module_hash(loaded: &mut LoadedManifest, wasm_hash: &HashRef) -> Result<()> {
    let module = loaded
        .modules
        .get_mut(REDUCER_NAME)
        .ok_or_else(|| anyhow!("module '{REDUCER_NAME}' missing from manifest"))?;
    module.wasm_hash = wasm_hash.clone();
    Ok(())
}
