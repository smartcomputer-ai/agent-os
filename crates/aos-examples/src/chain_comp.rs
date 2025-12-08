use std::path::Path;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::example_host::{ExampleHost, HarnessConfig};
use aos_host::adapters::mock::{MockHttpHarness, MockHttpResponse};

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
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: None,
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_PATH,
    })?;

    println!("→ Chain + Compensation demo");
    let start_event = ChainEventEnvelope::Start {
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
    };
    let ChainEventEnvelope::Start { order_id, .. } = &start_event;
    println!("     saga start → order_id={order_id}");
    host.send_event(&start_event)?;

    let mut http = MockHttpHarness::new();

    let mut requests = http.collect_requests(host.kernel_mut())?;
    if requests.len() != 1 {
        return Err(anyhow!(
            "expected 1 charge intent, found {}",
            requests.len()
        ));
    }
    let charge_ctx = requests.remove(0);
    println!("     responding to charge");
    http.respond_with(
        host.kernel_mut(),
        charge_ctx,
        MockHttpResponse::json(201, "{\"charge\":\"ok\"}"),
    )?;

    let mut requests = http.collect_requests(host.kernel_mut())?;
    if requests.len() != 1 {
        return Err(anyhow!(
            "expected 1 reserve intent after charge, found {}",
            requests.len()
        ));
    }
    let reserve_ctx = requests.remove(0);
    println!("     forcing reserve failure to trigger compensation");
    http.respond_with(
        host.kernel_mut(),
        reserve_ctx,
        MockHttpResponse::json(503, "{\"reserve\":\"error\"}"),
    )?;

    let mut requests = http.collect_requests(host.kernel_mut())?;
    if requests.len() != 1 {
        return Err(anyhow!(
            "expected refund intent after failure, found {}",
            requests.len()
        ));
    }
    let refund_ctx = requests.remove(0);
    println!("     refunding original charge");
    http.respond_with(
        host.kernel_mut(),
        refund_ctx,
        MockHttpResponse::json(202, "{\"refund\":\"ok\"}"),
    )?;

    let state: ChainStateView = host.read_state()?;
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

    host.finish()?.verify_replay()?;
    Ok(())
}
