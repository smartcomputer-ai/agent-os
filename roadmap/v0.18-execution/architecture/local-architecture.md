# Local And Embedded Architecture

## Goal

This note describes the local and embedded execution architecture that is now implemented in the
codebase.

The relevant crates are:

- `crates/aos-node/src/embedded`
- `crates/aos-node-local`

This is no longer a target-state sketch. It documents the structure that exists after the
`aos-node` / `aos-node-local` implementation work.

## Crate Split

The current split is:

- `aos-node`
  The reusable local/embedded runtime crate. It owns shared node models and the real
  local execution code under `src/embedded/`.
- `aos-node-local`
  Packaging around that runtime: CLI entrypoints, HTTP serving, and batch commands.
- `EmbeddedWorldHarness`
  A thin dev/test wrapper over the same local runtime surface, built on `LocalControl::open_batch`.

That means the architecture center is now `aos-node`, not `aos-node-local`.

## Relation To The Generic Execution Architecture

Local and embedded keep the same core execution seam as hosted:

- the kernel stays synchronous,
- append happens before opened async effects are published,
- internal deterministic effects stay inline,
- timers and external effects return continuations through normal world input,
- restart is reconstructed from durable state plus rehydrated open work.

What local removes is the hosted transport shell:

- no broker,
- no materializer,
- no journal-writer task,
- no worker/partition topology,
- no per-world Tokio runtime.

## Public Entry Points

The local public entrypoints are:

- `LocalControl::open(state_root)`
  Opens server mode with an internally owned Tokio edge runtime.
- `LocalControl::open_with_handle(state_root, handle)`
  Opens server mode using an existing Tokio runtime handle.
- `LocalControl::open_batch(state_root)`
  Opens direct mode with no scheduler thread.
- `aos-node-local serve`
  Builds a Tokio runtime, opens `LocalControl` with that handle, and serves the shared HTTP
  router.
- `aos-node-local batch`
  Opens batch/direct control and runs one-off local operations inline.

## Real Execution Center

### `LocalControl`

`LocalControl` is the public control facade. It currently has two modes:

- `Direct(Arc<LocalRuntime>)`
- `Server(LocalServerControl { runtime, scheduler_tx })`

In server mode, world-mutating work is serialized by sending closure jobs over a
`std::sync::mpsc` channel to a dedicated scheduler thread.

In direct mode, the same `LocalRuntime` methods are called inline on the caller thread.

There is no standalone `LocalScheduler` type in the current code. The scheduler role is split
between:

- the server-mode scheduler thread owned by `LocalControl`
- `LocalRuntime::process_all_pending()`

### `LocalRuntime`

`LocalRuntime` is the real embedded runtime. It owns:

- `LocalStatePaths`
- local CAS/blob planes
- SQLite-backed runtime planes
- world/effect/kernel configuration
- an `EdgeRuntime` that is either owned or borrowed from the caller
- Tokio `mpsc` lanes for effect continuations and timer wakes
- a mutex-protected `RuntimeState`

This is the concrete center of local execution today.

### `RuntimeState`

`RuntimeState` currently contains:

- `LocalSqlitePlanes`
- `next_submission_seq`
- `next_frame_offset`
- `worlds: BTreeMap<WorldId, WorldSlot>`
- `ready_worlds: VecDeque<WorldId>`

### `WorldSlot`

Each loaded world is a `WorldSlot` containing:

- identity and creation metadata
- `active_baseline`
- `next_world_seq`
- one `Kernel<FsCas>`
- one `EffectRuntime<WorldId>`
- one `TimerScheduler`
- `scheduled_timers` for dedupe
- one mailbox: `VecDeque<SubmissionEnvelope>`
- one `ready` bit
- in-memory `command_records`

This is loaded world state, not a runner task.

## Durable Local State

`LocalStatePaths` gives one local root, usually `.aos`, with:

- `runtime.sqlite3`
- `cas/`
- `cache/modules`
- `cache/wasmtime`
- `run/`
- `logs/`

`LocalSqlitePlanes` persists:

- `runtime_meta`
- `world_directory`
- `checkpoint_heads`
- `journal_frames`
- `command_projection`

The authoritative restart source is:

- the stored active baseline in `checkpoint_heads`
- plus per-world `journal_frames`
- plus CAS/blob content under the same root

There is no separate local materializer or replay transport layer.

## Submission And Scheduling Model

Server mode uses:

- one scheduler thread: `aos-local-scheduler`
- one effect bridge thread: `aos-local-effect-bridge`
- one timer bridge thread: `aos-local-timer-bridge`
- one closure queue: `std::sync::mpsc::Sender<SchedulerCommand>`

Inside `LocalRuntime`, scheduling is mailbox + ready-queue based:

1. enqueue a `SubmissionEnvelope` into the target world's mailbox
2. set the world's `ready` bit
3. push the world id onto `ready_worlds` only once
4. `process_all_pending()` drains async continuation channels, pops one ready world, pops one
   mailbox item, services it, and requeues the world if its mailbox is still non-empty

One ready-world turn processes one queued submission, but that submission may recursively generate
follow-up `WorldInput`s, and those follow-ups are drained before the service call returns.

The current scheduler queue is closure-based, not a typed `SchedulerMsg` enum.

## Control Semantics

The logical split is still useful:

- `WorldInput`
  Domain events, effect receipts, and effect stream frames
- `WorldControl`
  Governance commands
- `HostControl`
  World creation and world forking

In current local code, that split is represented by:

- `SubmissionEnvelope` in the per-world mailbox
- direct `create_world` / `fork_world` calls for host-scoped control
- command special-casing in `process_submission_locked()`

So the boundary is semantic, but not yet modeled as a public typed scheduler message lane.

## Admission And Commit Boundary

For a normal world input, the current local flow is:

1. record `tail_start = kernel.journal_head()`
2. `kernel.accept(input)`
3. `kernel.drain_until_idle_from(tail_start)`
4. if `KernelDrain.tail` is non-empty, build one `WorldLogFrame`
5. append that frame to SQLite
6. persist runtime counters and checkpoint-head metadata
7. compact the in-memory kernel journal through the active baseline height
8. classify opened effects and dispatch their post-append work

Governance commands use the same append boundary, except the control payload is first translated
through `run_governance_command()` before the drain.

World creation is explicit:

1. build a new `WorldSlot` from a manifest or seed
2. persist `world_directory`
3. persist the initial frame if boot emitted journal records, otherwise persist the checkpoint head
4. rehydrate open work for that world

## Post-Append Execution Classes

Opened effects are classified using the world's `EffectRuntime`:

- `InlineInternal`
  Executed immediately via `kernel.handle_internal_intent()`, producing follow-up receipts that
  stay on the same local service path.
- `OwnerLocalTimer`
  Recorded in `TimerScheduler`, deduped by `scheduled_timers`, and backed by Tokio sleeps that
  later re-enter as receipts.
- `ExternalAsync`
  Started via `EffectRuntime::ensure_started()`, with continuations returning as
  `EffectRuntimeEvent::WorldInput`.

This keeps the local durability rule intact: async work is only published after the frame that
opened it has already been durably appended.

## Execution Modes

### Server Mode

Used by:

- `aos-node-local serve`
- any caller using `LocalControl::open()` or `LocalControl::open_with_handle()`

Properties:

- HTTP requests run on Tokio when present
- `LocalControl` sends closure jobs to the scheduler thread and waits for the reply
- event and receipt submissions immediately call `process_all_pending()` before replying
- timers and external async continuations continue in the background through the effect/timer
  bridge threads

So server mode is async at the edges, but request handling is still synchronous with respect to the
local durable state transition it triggered.

### Direct Mode

Used by:

- `aos-node-local batch`
- `EmbeddedWorldHarness`
- callers using `LocalControl::open_batch()`

Properties:

- no scheduler thread
- no effect bridge thread
- no timer bridge thread
- the caller thread invokes `LocalRuntime` directly
- the runtime still owns an internal Tokio edge runtime for timers and external async effects
- continuation channels are drained opportunistically when methods call `process_all_pending()`

`step_world()` currently calls `process_all_pending()` for the whole runtime and then returns one
world summary. It is a catch-up hook, not a per-world isolated single-step primitive.

## Restart And Rehydration

On open, `LocalRuntime`:

1. loads world-directory rows and checkpoint heads from SQLite
2. reloads each world's initial manifest from CAS
3. rebuilds the kernel from the stored active baseline plus the world frame log
4. reloads command projections
5. scans `pending_workflow_receipts_snapshot()`
6. re-schedules timers and restarts external async work
7. runs `process_all_pending()` once after hot-world load

No in-memory mailbox or bridge-thread state is authoritative across restart.
Recovery comes from:

- SQLite
- CAS/blob content
- kernel-reconstructible open work

## Read Surfaces

There is no separate local materializer.

Current read paths are simple:

- world/runtime/state/manifest/journal/trace queries read from loaded runtime state
- command records are mirrored into SQLite command projection
- workspace and blob helpers operate directly against local CAS/workspace state

This is intentionally smaller than hosted.

## Current Scope And Gaps

Current local scope includes:

- durable create/fork
- domain-event ingress
- command and governance submission
- journal/state/manifest/trace queries
- CAS and workspace helper surfaces
- timer and external async continuation/restart plumbing

Current local gaps:

- secret binding/version control APIs still return
  `not_implemented("local node secret vault")`
- there is no public typed `LocalScheduler` / `SchedulerMsg` layer yet; the server scheduler lane
  is closure-based

## Tokio Topology

The Tokio and thread ownership model is documented in [tokio.md](./tokio.md).

The important local point is:

- Tokio owns async edges
- the scheduler owns serialized world progression
- direct mode remains first-class

## Summary

The implemented local/embedded architecture is now:

- `aos-node` as the reusable runtime crate
- `aos-node-local` as a packaging crate
- `LocalControl` as the public facade
- `LocalRuntime` as the actual execution center
- per-world `WorldSlot`s managed through one mailbox/ready-queue scheduler model
- inline SQLite frame append and checkpoint-head persistence
- Tokio used only for HTTP/effect/timer edges, not for kernel ownership
