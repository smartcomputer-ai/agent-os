#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use aos_sys::{WorkspaceCommit, WorkspaceCommitMeta};
use aos_wasm_sdk::{ReduceError, Reducer, ReducerCtx, aos_reducer, aos_variant};
use serde::{Deserialize, Serialize};

const WORKSPACE_SEED_SCHEMA: &str = "demo/WorkspaceSeed@1";
const WORKSPACE_COMMIT_SCHEMA: &str = "sys/WorkspaceCommit@1";

aos_reducer!(WorkspaceDemo);

#[derive(Default)]
struct WorkspaceDemo;

impl Reducer for WorkspaceDemo {
    type State = WorkspaceState;
    type Event = WorkspaceEvent;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
        match event {
            WorkspaceEvent::Start(start) => handle_start(ctx, start),
            WorkspaceEvent::Seeded(seeded) => handle_seeded(ctx, seeded),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct WorkspaceState {
    workspaces: BTreeMap<String, WorkspaceSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceSummary {
    version: Option<u64>,
    root_hash: String,
    entry_count: u64,
    diff_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceStart {
    workspaces: Vec<String>,
    owner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceSeed {
    workspace: String,
    owner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceSeeded {
    workspace: String,
    expected_head: Option<u64>,
    root_hash: String,
    entry_count: u64,
    diff_count: u64,
    owner: String,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum WorkspaceEvent {
        Start(WorkspaceStart),
        Seeded(WorkspaceSeeded),
    }
}

fn handle_start(ctx: &mut ReducerCtx<WorkspaceState, ()>, start: WorkspaceStart) {
    for workspace in start.workspaces {
        let seed = WorkspaceSeed {
            workspace,
            owner: start.owner.clone(),
        };
        ctx.intent(WORKSPACE_SEED_SCHEMA)
            .payload(&seed)
            .send();
    }
}

fn handle_seeded(ctx: &mut ReducerCtx<WorkspaceState, ()>, seeded: WorkspaceSeeded) {
    if let Some(existing) = ctx.state.workspaces.get(&seeded.workspace) {
        if existing.root_hash == seeded.root_hash
            && existing.entry_count == seeded.entry_count
            && existing.diff_count == seeded.diff_count
        {
            return;
        }
    }
    let version = match seeded.expected_head {
        Some(v) => Some(v.saturating_add(1)),
        None => ctx
            .state
            .workspaces
            .get(&seeded.workspace)
            .and_then(|summary| summary.version)
            .map(|v| v.saturating_add(1))
            .or(Some(1)),
    };
    let summary = WorkspaceSummary {
        version,
        root_hash: seeded.root_hash.clone(),
        entry_count: seeded.entry_count,
        diff_count: seeded.diff_count,
    };
    ctx.state
        .workspaces
        .insert(seeded.workspace.clone(), summary);

    let meta = WorkspaceCommitMeta {
        root_hash: seeded.root_hash,
        owner: seeded.owner,
        created_at: ctx.now_ns().unwrap_or(0),
    };
    let commit = WorkspaceCommit {
        workspace: seeded.workspace,
        expected_head: seeded.expected_head,
        meta,
    };
    ctx.intent(WORKSPACE_COMMIT_SCHEMA)
        .payload(&commit)
        .send();
}
