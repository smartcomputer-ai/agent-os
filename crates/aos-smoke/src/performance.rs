use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, ensure};
use aos_air_types::HashRef;
use aos_authoring::{manifest_loader, patch_modules};
use aos_kernel::journal::Journal;
use aos_kernel::{Kernel, MemStore, Store};
use aos_node::FsCas;
use serde::{Deserialize, Serialize};

use crate::example_host::{EventDispatchTiming, ExampleHost, HarnessConfig};

const WORKFLOW_NAME: &str = "demo/PerfCounter@1";
const EVENT_SCHEMA: &str = "demo/PerfEvent@1";
const MODULE_CRATE: &str = "crates/aos-smoke/fixtures/12-performance/workflow";
const NON_KEYED_ASSETS: &str = "air.non_keyed";
const KEYED_ASSETS: &str = "air.keyed";

pub const DEFAULT_MESSAGES: u64 = 100;
pub const DEFAULT_CELLS: u64 = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PerfEvent {
    cell: String,
    inc: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PerfState {
    count: u64,
}

struct PerfMetrics {
    startup: Duration,
    elapsed: Duration,
    events: u64,
}

#[derive(Default)]
struct ScopeTimers {
    encode: Duration,
    submit: Duration,
    drain: Duration,
    validation: Duration,
    replay: Duration,
}

impl ScopeTimers {
    fn add_dispatch(&mut self, timing: EventDispatchTiming) {
        self.encode += timing.encode;
        self.submit += timing.submit;
        self.drain += timing.drain;
    }

    fn dispatch_scoped_total(&self) -> Duration {
        self.encode + self.submit + self.drain
    }
}

struct MemPerfHost {
    kernel: Kernel<MemStore>,
    workflow_name: String,
    event_schema: String,
}

impl MemPerfHost {
    fn prepare(example_root: &Path, assets_root: &Path) -> Result<Self> {
        let wasm_bytes = crate::util::compile_workflow(MODULE_CRATE)?;
        let store = Arc::new(MemStore::new());
        let wasm_hash = store
            .put_blob(&wasm_bytes)
            .context("store workflow wasm blob in memory")?;
        let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).context("hash workflow wasm")?;

        let paths = crate::util::local_state_paths(example_root);
        let fs_store = Arc::new(FsCas::open_with_paths(&paths).context("open fixture CAS")?);
        let mut loaded = manifest_loader::load_from_assets_with_imports(fs_store, assets_root, &[])
            .context("load manifest from assets")?
            .ok_or_else(|| anyhow!("example manifest missing at {}", assets_root.display()))?;

        let module_name = loaded
            .ops
            .get(WORKFLOW_NAME)
            .map(|op| op.implementation.module.as_str())
            .unwrap_or(WORKFLOW_NAME)
            .to_string();
        let patched = patch_modules(&mut loaded, &wasm_hash_ref, |name, _| name == module_name);
        if patched == 0 {
            anyhow::bail!(
                "module '{module_name}' for workflow '{WORKFLOW_NAME}' missing from manifest"
            );
        }

        let kernel = Kernel::from_loaded_manifest_with_config(
            store.clone(),
            loaded,
            Journal::new(),
            crate::util::kernel_config(example_root)?,
        )
        .context("create in-memory test host")?;

        Ok(Self {
            kernel,
            workflow_name: WORKFLOW_NAME.to_string(),
            event_schema: EVENT_SCHEMA.to_string(),
        })
    }

    fn send_event(&mut self, event: &PerfEvent) -> Result<()> {
        self.send_event_timed(event).map(|_| ())
    }

    fn send_event_timed(&mut self, event: &PerfEvent) -> Result<EventDispatchTiming> {
        let encode_start = Instant::now();
        let cbor = serde_cbor::to_vec(event)?;
        let encode = encode_start.elapsed();

        let submit_start = Instant::now();
        self.kernel
            .submit_domain_event_result(self.event_schema.clone(), cbor)
            .context("send event")?;
        let submit = submit_start.elapsed();

        let drain_start = Instant::now();
        self.kernel.tick_until_idle().context("drain after event")?;
        let drain = drain_start.elapsed();

        Ok(EventDispatchTiming {
            encode,
            submit,
            drain,
        })
    }

    fn read_state(&self) -> Result<PerfState> {
        let bytes = self
            .kernel
            .workflow_state(&self.workflow_name)
            .ok_or_else(|| anyhow!("read workflow state"))?;
        serde_cbor::from_slice(&bytes).context("decode workflow state")
    }

    fn workflow_state_bytes(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.kernel
            .workflow_state_bytes(&self.workflow_name, Some(key))
            .context("read keyed workflow state bytes")
    }
}

pub fn run(example_root: &Path) -> Result<()> {
    run_with_config(example_root, DEFAULT_MESSAGES, DEFAULT_CELLS, false)
}

pub fn run_with_config(
    example_root: &Path,
    messages: u64,
    cells: u64,
    in_memory: bool,
) -> Result<()> {
    ensure!(cells > 0, "cells must be >= 1");

    let mode = if in_memory { "in-memory" } else { "fs" };
    println!("→ Performance demo (messages={messages}, cells={cells}, mode={mode})");

    let (non_keyed, keyed) = if in_memory {
        (
            run_non_keyed_in_memory(example_root, messages)?,
            run_keyed_in_memory(example_root, messages, cells)?,
        )
    } else {
        (
            run_non_keyed_fs(example_root, messages)?,
            run_keyed_fs(example_root, messages, cells)?,
        )
    };

    let total_events = non_keyed.events.saturating_add(keyed.events);
    let total_elapsed = non_keyed.elapsed + keyed.elapsed;
    let total_startup = non_keyed.startup + keyed.startup;

    println!("   startup time: total={:.3}s", total_startup.as_secs_f64());
    println!(
        "   total perf: events={}, elapsed={:.3}s, events/sec={:.2}",
        total_events,
        total_elapsed.as_secs_f64(),
        events_per_second(total_events, total_elapsed)
    );

    Ok(())
}

fn run_non_keyed_fs(example_root: &Path, messages: u64) -> Result<PerfMetrics> {
    let assets_root = example_root.join(NON_KEYED_ASSETS);

    let startup_begin = Instant::now();
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: Some(assets_root.as_path()),
        workflow_name: WORKFLOW_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_CRATE,
    })?;

    // Warm up the module so startup and steady-state timing are separated.
    host.send_event(&PerfEvent {
        cell: String::new(),
        inc: 0,
    })?;
    let startup = startup_begin.elapsed();

    // Scope timers intentionally start after warmup.
    let mut scopes = ScopeTimers::default();
    let begin = Instant::now();
    for _ in 0..messages {
        let timing = host.send_event_timed(&PerfEvent {
            cell: String::new(),
            inc: 1,
        })?;
        scopes.add_dispatch(timing);
    }
    let elapsed = begin.elapsed();

    let validate_begin = Instant::now();
    let state: PerfState = host.read_state()?;
    ensure!(
        state.count == messages,
        "non-keyed count mismatch: expected {}, got {}",
        messages,
        state.count
    );
    scopes.validation += validate_begin.elapsed();

    println!("   non-keyed startup: {:.3}s", startup.as_secs_f64());
    println!(
        "   non-keyed perf: events={}, elapsed={:.3}s, events/sec={:.2}",
        messages,
        elapsed.as_secs_f64(),
        events_per_second(messages, elapsed)
    );
    print_scope_timers("non-keyed", &scopes, elapsed);

    let replay_begin = Instant::now();
    host.finish()?.verify_replay()?;
    scopes.replay += replay_begin.elapsed();
    print_tail_timers("non-keyed", &scopes);

    Ok(PerfMetrics {
        startup,
        elapsed,
        events: messages,
    })
}

fn run_keyed_fs(example_root: &Path, messages: u64, cells: u64) -> Result<PerfMetrics> {
    let assets_root = example_root.join(KEYED_ASSETS);
    let cell_ids: Vec<String> = (0..cells).map(cell_id).collect();
    let mut expected_counts = vec![0_u64; cells as usize];

    let startup_begin = Instant::now();
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: Some(assets_root.as_path()),
        workflow_name: WORKFLOW_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_CRATE,
    })?;

    // Warm up keyed routing and module load.
    host.send_event(&PerfEvent {
        cell: cell_ids[0].clone(),
        inc: 0,
    })?;
    let startup = startup_begin.elapsed();

    // Scope timers intentionally start after warmup.
    let mut scopes = ScopeTimers::default();
    let begin = Instant::now();
    for seq in 0..messages {
        let idx = (seq % cells) as usize;
        expected_counts[idx] = expected_counts[idx].saturating_add(1);
        let timing = host.send_event_timed(&PerfEvent {
            cell: cell_ids[idx].clone(),
            inc: 1,
        })?;
        scopes.add_dispatch(timing);
    }
    let elapsed = begin.elapsed();

    let validate_begin = Instant::now();
    let mut observed_cells = vec![false; cells as usize];
    let mut observed_key_samples = Vec::new();
    let mut total_count = 0_u64;

    let cell_meta = host.kernel_mut().list_cells(WORKFLOW_NAME)?;
    for meta in cell_meta {
        observed_key_samples.push(meta.key_bytes.clone());

        let key_text: String =
            serde_cbor::from_slice(&meta.key_bytes).context("decode keyed cell key")?;
        let idx = parse_cell_index(&key_text, cells)? as usize;

        let state_bytes = host
            .kernel_mut()
            .workflow_state_bytes(WORKFLOW_NAME, Some(&meta.key_bytes))
            .context("read keyed workflow state bytes")?
            .ok_or_else(|| anyhow!("missing keyed state for '{key_text}'"))?;
        let state: PerfState =
            serde_cbor::from_slice(&state_bytes).context("decode keyed workflow state")?;

        let expected = expected_counts[idx];
        ensure!(
            state.count == expected,
            "keyed count mismatch for '{}': expected {}, got {}",
            key_text,
            expected,
            state.count
        );

        observed_cells[idx] = true;
        total_count = total_count.saturating_add(state.count);
    }

    for (idx, expected) in expected_counts.iter().enumerate() {
        if *expected > 0 {
            ensure!(
                observed_cells[idx],
                "missing keyed state for '{}'",
                cell_ids[idx]
            );
        }
    }
    ensure!(
        observed_cells[0],
        "missing keyed warmup cell '{}'",
        cell_ids[0]
    );
    ensure!(
        total_count == messages,
        "keyed total mismatch: expected {}, got {}",
        messages,
        total_count
    );
    scopes.validation += validate_begin.elapsed();

    println!("   keyed startup: {:.3}s", startup.as_secs_f64());
    println!(
        "   keyed perf: events={}, elapsed={:.3}s, events/sec={:.2}",
        messages,
        elapsed.as_secs_f64(),
        events_per_second(messages, elapsed)
    );
    print_scope_timers("keyed", &scopes, elapsed);

    let replay_begin = Instant::now();
    host.finish_with_keyed_samples(Some(WORKFLOW_NAME), &observed_key_samples)?
        .verify_replay()?;
    scopes.replay += replay_begin.elapsed();
    print_tail_timers("keyed", &scopes);

    Ok(PerfMetrics {
        startup,
        elapsed,
        events: messages,
    })
}

fn run_non_keyed_in_memory(example_root: &Path, messages: u64) -> Result<PerfMetrics> {
    let assets_root = example_root.join(NON_KEYED_ASSETS);

    let startup_begin = Instant::now();
    let mut host = MemPerfHost::prepare(example_root, assets_root.as_path())?;

    // Warm up the module so startup and steady-state timing are separated.
    host.send_event(&PerfEvent {
        cell: String::new(),
        inc: 0,
    })?;
    let startup = startup_begin.elapsed();

    // Scope timers intentionally start after warmup.
    let mut scopes = ScopeTimers::default();
    let begin = Instant::now();
    for _ in 0..messages {
        let timing = host.send_event_timed(&PerfEvent {
            cell: String::new(),
            inc: 1,
        })?;
        scopes.add_dispatch(timing);
    }
    let elapsed = begin.elapsed();

    let validate_begin = Instant::now();
    let state = host.read_state()?;
    ensure!(
        state.count == messages,
        "non-keyed count mismatch: expected {}, got {}",
        messages,
        state.count
    );
    scopes.validation += validate_begin.elapsed();

    println!("   non-keyed startup: {:.3}s", startup.as_secs_f64());
    println!(
        "   non-keyed perf: events={}, elapsed={:.3}s, events/sec={:.2}",
        messages,
        elapsed.as_secs_f64(),
        events_per_second(messages, elapsed)
    );
    print_scope_timers("non-keyed", &scopes, elapsed);
    println!("   non-keyed replay: skipped (in-memory mode)");
    println!();

    Ok(PerfMetrics {
        startup,
        elapsed,
        events: messages,
    })
}

fn run_keyed_in_memory(example_root: &Path, messages: u64, cells: u64) -> Result<PerfMetrics> {
    let assets_root = example_root.join(KEYED_ASSETS);
    let cell_ids: Vec<String> = (0..cells).map(cell_id).collect();
    let mut expected_counts = vec![0_u64; cells as usize];

    let startup_begin = Instant::now();
    let mut host = MemPerfHost::prepare(example_root, assets_root.as_path())?;

    // Warm up keyed routing and module load.
    host.send_event(&PerfEvent {
        cell: cell_ids[0].clone(),
        inc: 0,
    })?;
    let startup = startup_begin.elapsed();

    // Scope timers intentionally start after warmup.
    let mut scopes = ScopeTimers::default();
    let begin = Instant::now();
    for seq in 0..messages {
        let idx = (seq % cells) as usize;
        expected_counts[idx] = expected_counts[idx].saturating_add(1);
        let timing = host.send_event_timed(&PerfEvent {
            cell: cell_ids[idx].clone(),
            inc: 1,
        })?;
        scopes.add_dispatch(timing);
    }
    let elapsed = begin.elapsed();

    let validate_begin = Instant::now();
    let mut observed_cells = vec![false; cells as usize];
    let mut total_count = 0_u64;

    for meta in host.kernel.list_cells(WORKFLOW_NAME)? {
        let key_text: String =
            serde_cbor::from_slice(&meta.key_bytes).context("decode keyed cell key")?;
        let idx = parse_cell_index(&key_text, cells)? as usize;

        let state_bytes = host
            .workflow_state_bytes(&meta.key_bytes)?
            .ok_or_else(|| anyhow!("missing keyed state for '{key_text}'"))?;
        let state: PerfState =
            serde_cbor::from_slice(&state_bytes).context("decode keyed workflow state")?;

        let expected = expected_counts[idx];
        ensure!(
            state.count == expected,
            "keyed count mismatch for '{}': expected {}, got {}",
            key_text,
            expected,
            state.count
        );

        observed_cells[idx] = true;
        total_count = total_count.saturating_add(state.count);
    }

    for (idx, expected) in expected_counts.iter().enumerate() {
        if *expected > 0 {
            ensure!(
                observed_cells[idx],
                "missing keyed state for '{}'",
                cell_ids[idx]
            );
        }
    }
    ensure!(
        observed_cells[0],
        "missing keyed warmup cell '{}'",
        cell_ids[0]
    );
    ensure!(
        total_count == messages,
        "keyed total mismatch: expected {}, got {}",
        messages,
        total_count
    );
    scopes.validation += validate_begin.elapsed();

    println!("   keyed startup: {:.3}s", startup.as_secs_f64());
    println!(
        "   keyed perf: events={}, elapsed={:.3}s, events/sec={:.2}",
        messages,
        elapsed.as_secs_f64(),
        events_per_second(messages, elapsed)
    );
    print_scope_timers("keyed", &scopes, elapsed);
    println!("   keyed replay: skipped (in-memory mode)");
    println!();

    Ok(PerfMetrics {
        startup,
        elapsed,
        events: messages,
    })
}

fn parse_cell_index(key: &str, cells: u64) -> Result<u64> {
    let raw = key
        .strip_prefix("cell-")
        .ok_or_else(|| anyhow!("unexpected keyed cell id '{key}'"))?;
    let idx: u64 = raw
        .parse()
        .with_context(|| format!("parse keyed cell index from '{key}'"))?;
    ensure!(
        idx < cells,
        "keyed cell index out of range: {} >= {}",
        idx,
        cells
    );
    Ok(idx)
}

fn cell_id(idx: u64) -> String {
    format!("cell-{idx}")
}

fn events_per_second(events: u64, elapsed: Duration) -> f64 {
    let seconds = elapsed.as_secs_f64();
    if seconds <= 0.0 {
        return 0.0;
    }
    events as f64 / seconds
}

fn print_scope_timers(label: &str, scopes: &ScopeTimers, loop_wall: Duration) {
    println!(
        "   {label} scopes (post-warmup): encode={:.3}s submit={:.3}s drain={:.3}s scoped_dispatch_total={:.3}s loop_wall={:.3}s validation={:.3}s",
        scopes.encode.as_secs_f64(),
        scopes.submit.as_secs_f64(),
        scopes.drain.as_secs_f64(),
        scopes.dispatch_scoped_total().as_secs_f64(),
        loop_wall.as_secs_f64(),
        scopes.validation.as_secs_f64(),
    );
}

fn print_tail_timers(label: &str, scopes: &ScopeTimers) {
    println!(
        "   {label} scopes (post-warmup tail): replay={:.3}s",
        scopes.replay.as_secs_f64()
    );
}
