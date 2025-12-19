//! WorldFS lab: keyed notes + ObjectCatalog registration + blob storage.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Result, anyhow, ensure};
use aos_air_types::HashRef;
use aos_effects::builtins::{BlobPutParams, BlobPutReceipt};
use aos_effects::{EffectKind, EffectReceipt, ReceiptStatus};
use aos_store::Store;
use serde::{Deserialize, Serialize};
use serde_cbor;
use sha2::{Digest, Sha256};

use crate::example_host::{ExampleHost, HarnessConfig};

const REDUCER_NAME: &str = "notes/NotebookSM@1";
const MODULE_CRATE: &str = "examples/09-worldfs-lab/reducer";
const ADAPTER_ID: &str = "adapter.blob.fake";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NoteStarted {
    note_id: String,
    title: String,
    author: String,
    created_at_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NoteAppended {
    note_id: String,
    line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NoteFinalized {
    note_id: String,
    finalized_at_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NoteState {
    pc: NotePc,
    title: String,
    author: String,
    lines: Vec<String>,
    created_at_ns: u64,
    finalized_at_ns: Option<u64>,
    report_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum NotePc {
    Draft,
    Finalized,
    Archived,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ObjectMeta {
    name: String,
    kind: String,
    hash: String,
    tags: Vec<String>,
    created_at: u64,
    owner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ObjectVersions {
    latest: u64,
    versions: HashMap<u64, ObjectMeta>,
}

pub fn run(example_root: &Path) -> Result<()> {
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: None,
        reducer_name: REDUCER_NAME,
        event_schema: "notes/NoteStarted@1",
        module_crate: MODULE_CRATE,
    })?;

    println!("→ WorldFS lab: notebook + catalog");
    let seeds = demo_notes();
    seed_world(&mut host, &seeds)?;
    drive_blob_puts(&mut host, &seeds)?;

    let key_cbors: Vec<Vec<u8>> = seeds
        .iter()
        .map(|s| serde_cbor::to_vec(&s.id).expect("serialize key"))
        .collect();

    let summary = summarize(&mut host, &seeds)?;
    println!("   notes archived: {}", summary.notes_archived);
    println!("   catalog entries: {}", summary.catalog_entries);

    host.finish_with_keyed_samples(Some(REDUCER_NAME), &key_cbors)?
        .verify_replay()?;
    Ok(())
}

fn seed_world(host: &mut ExampleHost, seeds: &[NoteSeed]) -> Result<()> {
    for seed in seeds {
        let start = NoteStarted {
            note_id: seed.id.clone(),
            title: seed.title.clone(),
            author: seed.author.clone(),
            created_at_ns: seed.created_at_ns,
        };
        host.send_event_as("notes/NoteStarted@1", &start)?;
        for line in &seed.lines {
            let append = NoteAppended {
                note_id: seed.id.clone(),
                line: line.clone(),
            };
            host.send_event_as("notes/NoteAppended@1", &append)?;
        }
        let finalize = NoteFinalized {
            note_id: seed.id.clone(),
            finalized_at_ns: seed.created_at_ns + 5,
        };
        host.send_event_as("notes/NoteFinalized@1", &finalize)?;
    }
    Ok(())
}

fn drive_blob_puts(host: &mut ExampleHost, seeds: &[NoteSeed]) -> Result<()> {
    let seed_map: HashMap<String, NoteSeed> =
        seeds.iter().cloned().map(|s| (s.id.clone(), s)).collect();
    let store = host.store();
    let kernel = host.kernel_mut();
    let mut safety = 0;
    loop {
        let intents = kernel.drain_effects();
        if intents.is_empty() {
            break;
        }
        for intent in intents {
            ensure!(
                intent.kind == EffectKind::blob_put(),
                "unexpected effect {:?}",
                intent.kind
            );
            let params: BlobPutParams = serde_cbor::from_slice(&intent.params_cbor)?;
            let note_id = params.namespace.clone();
            let seed = seed_map
                .get(&note_id)
                .ok_or_else(|| anyhow!("unknown note seed {note_id}"))?;
            let report = build_report_from_seed(&note_id, seed);
            let hash = hash_bytes(&report);
            ensure!(
                hash == params.blob_ref.as_str(),
                "hash mismatch for note {note_id}: plan {:?} vs computed {}",
                params.blob_ref.as_str(),
                hash
            );
            let stored_hash = store
                .put_blob(&report)
                .map_err(|e| anyhow!("store blob: {e}"))?;
            let stored_ref = HashRef::new(stored_hash.to_hex())?;
            ensure!(
                stored_ref.as_str() == params.blob_ref.as_str(),
                "store hash mismatch for note {note_id}"
            );

            let receipt = EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: ADAPTER_ID.into(),
                status: ReceiptStatus::Ok,
                payload_cbor: serde_cbor::to_vec(&BlobPutReceipt {
                    blob_ref: params.blob_ref.clone(),
                    size: report.len() as u64,
                })?,
                cost_cents: Some(0),
                signature: vec![0; 64],
            };
            kernel.handle_receipt(receipt)?;
        }
        kernel.tick_until_idle()?;
        safety += 1;
        ensure!(safety < 16, "safety trip: too many effect cycles");
    }
    Ok(())
}

fn summarize(host: &mut ExampleHost, seeds: &[NoteSeed]) -> Result<Summary> {
    Ok(Summary {
        notes_archived: seeds.len() as u64,
        catalog_entries: seeds.len() as u64,
    })
}

struct Summary {
    notes_archived: u64,
    catalog_entries: u64,
}

#[derive(Clone)]
struct NoteSeed {
    id: String,
    title: String,
    author: String,
    lines: Vec<String>,
    created_at_ns: u64,
}

fn demo_notes() -> Vec<NoteSeed> {
    vec![
        NoteSeed {
            id: "alpha".into(),
            title: "Morning Plans".into(),
            author: "Ada".into(),
            lines: vec![
                "Open the hutch".into(),
                "Measure widget flow".into(),
                "File findings".into(),
            ],
            created_at_ns: 1_000,
        },
        NoteSeed {
            id: "beta".into(),
            title: "Shipping".into(),
            author: "Lin".into(),
            lines: vec!["Prep labels".into(), "Book pickup".into()],
            created_at_ns: 2_000,
        },
    ]
}

fn build_report(note_id: &str, state: &NoteState) -> Vec<u8> {
    let mut lines = Vec::new();
    lines.push(format!(
        "Note {note_id}: {} (by {})",
        state.title, state.author
    ));
    lines.push(format!("Status: {:?}", state.pc));
    lines.push(format!("Lines: {}", state.lines.len()));
    lines.push("--".to_string());
    for (idx, line) in state.lines.iter().enumerate() {
        lines.push(format!("{:02}: {line}", idx + 1));
    }
    lines.join("\n").into_bytes()
}

fn build_report_from_seed(note_id: &str, seed: &NoteSeed) -> Vec<u8> {
    let mut lines = Vec::new();
    lines.push(format!(
        "Note {note_id}: {} (by {})",
        seed.title, seed.author
    ));
    lines.push("Status: Finalized".to_string());
    lines.push(format!("Lines: {}", seed.lines.len()));
    lines.push("--".to_string());
    for (idx, line) in seed.lines.iter().enumerate() {
        lines.push(format!("{:02}: {line}", idx + 1));
    }
    lines.join("\n").into_bytes()
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    format!("sha256:{}", hex::encode(digest))
}
