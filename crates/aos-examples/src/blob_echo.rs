//! Blob Echo demo wired up through AIR assets and the reducer harness.
//!
//! Reducer emits `blob.put`/`blob.get` micro-effects; this runner drains the
//! intents, synthesizes receipts, and relies on the shared harness for setup and
//! deterministic replay.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Result, anyhow};
use aos_air_types::HashRef;
use aos_effects::builtins::{BlobGetParams, BlobGetReceipt, BlobPutParams, BlobPutReceipt};
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use aos_kernel::Kernel;
use aos_store::FsStore;
use serde::{Deserialize, Serialize};
use serde_cbor;
use sha2::{Digest, Sha256};

use crate::example_host::{ExampleHost, HarnessConfig};

const REDUCER_NAME: &str = "demo/BlobEchoSM@1";
const EVENT_SCHEMA: &str = "demo/BlobEchoEvent@1";
const ADAPTER_ID: &str = "adapter.blob.fake";

#[derive(Debug, Clone)]
struct BlobEchoInput {
    namespace: String,
    key: String,
    data: Vec<u8>,
}

#[derive(Default)]
struct BlobHarnessStore {
    pending_blobs: HashMap<String, Vec<u8>>,
    key_to_blob: HashMap<(String, String), String>,
}

pub fn run(example_root: &Path) -> Result<()> {
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: None,
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: "examples/02-blob-echo/reducer",
    })?;

    let input = BlobEchoInput {
        namespace: "demo".into(),
        key: "echo".into(),
        data: b"Blob Echo Example".to_vec(),
    };

    println!("→ Blob Echo demo");
    drive_blob_echo(&mut host, input)?;

    let final_state: ReducerEchoState = host.read_state()?;
    println!(
        "   final state: pc={:?}, stored_ref={:?}, retrieved_ref={:?}",
        final_state.pc, final_state.stored_blob_ref, final_state.retrieved_blob_ref
    );

    host.finish()?.verify_replay()?;
    Ok(())
}

fn drive_blob_echo(host: &mut ExampleHost, input: BlobEchoInput) -> Result<()> {
    let mut harness = BlobHarnessStore::default();
    let blob_ref = hash_bytes(&input.data);
    harness
        .pending_blobs
        .insert(blob_ref.clone(), input.data.clone());
    harness
        .key_to_blob
        .insert((input.namespace.clone(), input.key.clone()), blob_ref);

    let start_event = StartEvent {
        namespace: input.namespace,
        key: input.key,
        data: input.data,
    };
    host.send_event(&start_event)?;
    synthesize_blob_effects(host.kernel_mut(), &mut harness)
}

fn synthesize_blob_effects(
    kernel: &mut Kernel<FsStore>,
    harness: &mut BlobHarnessStore,
) -> Result<()> {
    loop {
        let intents = kernel.drain_effects();
        if intents.is_empty() {
            break;
        }
        for intent in intents {
            match intent.kind.as_str() {
                EffectKind::BLOB_PUT => handle_blob_put(kernel, harness, intent)?,
                EffectKind::BLOB_GET => handle_blob_get(kernel, harness, intent)?,
                other => return Err(anyhow!("unexpected effect {other}")),
            }
        }
    }
    Ok(())
}

fn handle_blob_put(
    kernel: &mut Kernel<FsStore>,
    harness: &mut BlobHarnessStore,
    intent: EffectIntent,
) -> Result<()> {
    let params: BlobPutParams = serde_cbor::from_slice(&intent.params_cbor)?;
    let blob_ref = params.blob_ref.as_str().to_string();
    let data = harness
        .pending_blobs
        .get(&blob_ref)
        .ok_or_else(|| anyhow!("missing blob data for {blob_ref}"))?;
    println!(
        "     blob.put -> namespace={} blob_ref={} size={} bytes",
        params.namespace,
        blob_ref,
        data.len()
    );

    let receipt_payload = BlobPutReceipt {
        blob_ref: params.blob_ref.clone(),
        size: data.len() as u64,
    };
    let receipt = EffectReceipt {
        intent_hash: intent.intent_hash,
        adapter_id: ADAPTER_ID.into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&receipt_payload)?,
        cost_cents: Some(0),
        signature: vec![0; 64],
    };
    kernel.handle_receipt(receipt)?;
    kernel.tick_until_idle()?;
    Ok(())
}

fn handle_blob_get(
    kernel: &mut Kernel<FsStore>,
    harness: &mut BlobHarnessStore,
    intent: EffectIntent,
) -> Result<()> {
    let params: BlobGetParams = serde_cbor::from_slice(&intent.params_cbor)?;
    let key = (params.namespace.clone(), params.key.clone());
    let blob_ref = harness.key_to_blob.get(&key).ok_or_else(|| {
        anyhow!(
            "no blob stored for namespace={} key={}",
            params.namespace,
            params.key
        )
    })?;
    let data = harness
        .pending_blobs
        .get(blob_ref)
        .ok_or_else(|| anyhow!("missing blob bytes for {blob_ref}"))?;
    println!(
        "     blob.get -> namespace={} key={} size={} bytes",
        params.namespace,
        params.key,
        data.len()
    );

    let receipt_payload = BlobGetReceipt {
        blob_ref: HashRef::new(blob_ref.clone()).map_err(|err| anyhow!("invalid hash: {err}"))?,
        size: data.len() as u64,
    };
    let receipt = EffectReceipt {
        intent_hash: intent.intent_hash,
        adapter_id: ADAPTER_ID.into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&receipt_payload)?,
        cost_cents: Some(0),
        signature: vec![0; 64],
    };
    kernel.handle_receipt(receipt)?;
    kernel.tick_until_idle()?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReducerEchoState {
    pc: ReducerPc,
    stored_blob_ref: Option<String>,
    retrieved_blob_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ReducerPc {
    Idle,
    Putting,
    Getting,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartEvent {
    namespace: String,
    key: String,
    #[serde(with = "serde_bytes")]
    data: Vec<u8>,
}

fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    format!("sha256:{}", hex::encode(digest))
}
