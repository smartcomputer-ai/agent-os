#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use aos_wasm_sdk::{
    ReduceError, Reducer, ReducerCtx, Value, aos_event_union, aos_reducer, aos_variant,
};
use serde::{Deserialize, Serialize};
use serde_cbor;
use sha2::{Digest, Sha256};

// Minimal mains to satisfy cargo for host/wasm builds.
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

const SNAPSHOT_REQUESTED: &str = "notes/SnapshotRequested@1";

aos_reducer!(NotebookSm);

#[derive(Default)]
struct NotebookSm;

aos_event_union! {
    #[derive(Debug, Clone, Serialize)]
    enum NoteEvent {
        Start(NoteStarted),
        Append(NoteAppended),
        Finalize(NoteFinalized),
        Archived(NoteArchived)
    }
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum NotePc {
        Draft,
        Finalized,
        Archived,
    }
}

impl Default for NotePc {
    fn default() -> Self {
        NotePc::Draft
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
struct NoteArchived {
    note_id: String,
    report_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SnapshotRequested {
    note_id: String,
    report_hash: String,
    namespace: String,
    object_path: String,
    title: String,
    author: String,
    line_count: u32,
    finalized_at_ns: u64,
    created_at: u64,
}

impl Reducer for NotebookSm {
    type State = NoteState;
    type Event = NoteEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        match event {
            NoteEvent::Start(ev) => handle_start(ctx, ev),
            NoteEvent::Append(ev) => handle_append(ctx, ev),
            NoteEvent::Finalize(ev) => handle_finalize(ctx, ev),
            NoteEvent::Archived(ev) => handle_archived(ctx, ev),
        }
        Ok(())
    }
}

fn handle_start(ctx: &mut ReducerCtx<NoteState>, ev: NoteStarted) {
    if !matches!(ctx.state.pc, NotePc::Draft | NotePc::Archived) {
        return;
    }
    ctx.state.pc = NotePc::Draft;
    ctx.state.title = ev.title;
    ctx.state.author = ev.author;
    ctx.state.lines.clear();
    ctx.state.created_at_ns = ev.created_at_ns;
    ctx.state.finalized_at_ns = None;
    ctx.state.report_hash = None;
}

fn handle_append(ctx: &mut ReducerCtx<NoteState>, ev: NoteAppended) {
    if !matches!(ctx.state.pc, NotePc::Draft) {
        return;
    }
    ctx.state.lines.push(ev.line);
}

fn handle_finalize(ctx: &mut ReducerCtx<NoteState>, ev: NoteFinalized) {
    if !matches!(ctx.state.pc, NotePc::Draft) {
        return;
    }
    ctx.state.pc = NotePc::Finalized;
    ctx.state.finalized_at_ns = Some(ev.finalized_at_ns);

    let note_id = decode_key(ctx.key());
    let report_bytes = build_report(&note_id, &ctx.state);
    let report_hash = hash_bytes(&report_bytes);
    ctx.state.report_hash = Some(report_hash.clone());

    let snapshot = SnapshotRequested {
        note_id: note_id.clone(),
        report_hash: report_hash.clone(),
        namespace: note_id.clone(),
        object_path: format!("notes/{}/report", note_id),
        title: ctx.state.title.clone(),
        author: ctx.state.author.clone(),
        line_count: ctx.state.lines.len() as u32,
        finalized_at_ns: ev.finalized_at_ns,
        created_at: ev.finalized_at_ns,
    };
    ctx.intent(SNAPSHOT_REQUESTED).payload(&snapshot).send();
}

fn handle_archived(ctx: &mut ReducerCtx<NoteState>, ev: NoteArchived) {
    if !matches!(ctx.state.pc, NotePc::Finalized) {
        return;
    }
    if ctx.state.report_hash.as_deref() == Some(&ev.report_hash) {
        ctx.state.pc = NotePc::Archived;
    }
}

fn decode_key(raw: Option<&[u8]>) -> String {
    raw.and_then(|b| serde_cbor::from_slice::<String>(b).ok())
        .unwrap_or_else(|| "unknown".to_string())
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

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    format!("sha256:{}", hex::encode(digest))
}
