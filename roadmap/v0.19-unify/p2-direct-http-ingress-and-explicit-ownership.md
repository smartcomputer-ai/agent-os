# P2: Direct HTTP Ingress and Explicit Ownership

**Priority**: P2  
**Effort**: Large  
**Risk if deferred**: High (the runtime remains mentally organized around Kafka consumer-group
ownership even though the intended hosted model is direct acceptance into a colocated worker)  
**Status**: Implemented (closeout remaining)  
**Depends on**:
- `roadmap/v0.19-unify/directive.md`
- `roadmap/v0.19-unify/p1-colocated-control-and-hot-world-reads.md`
- `roadmap/v0.18-execution/architecture/hosted-architecture.md`

## Goal

Define the second implementation phase for `v0.19` around one clear hosted model:

1. direct HTTP/control becomes the only hosted ingress path,
2. worker ownership becomes explicit runtime/config state instead of ingress-partition assignment,
3. the existing `accept -> enqueue -> stage slices -> flush -> post-commit` worker path remains the
   durable execution fence,
4. HTTP can optionally wait until the accepted item is durably flushed,
5. Kafka may remain as a journal backend for now, but Kafka ingress is removed as an architectural
   concept.

This phase is intentionally about removing the old transport-centered execution model.
It is not yet the full world-discovery/checkpoint redesign or the final switchable-journal phase.

## Why This Exists

P1 proved the control/read surface should point directly at the colocated runtime.

The next architectural mistake to remove is the assumption that worker ownership comes from ingress
partition assignment.

Today the hosted runtime still carries the old shape:

1. consumer-group assignment tells the worker which ingress partitions it owns,
2. ingress partitions imply which worlds it may activate and service,
3. Kafka source acknowledgement is coupled to the same assignment mechanism,
4. direct HTTP acceptance is forced to fit around that old center.

That makes the execution model harder to understand than it needs to be.

The intended hosted model is simpler:

1. the runtime is configured to own a set of worlds or a world inventory,
2. callers submit directly to that runtime over HTTP/control,
3. accepted work enters the worker mailbox/stage/flush path,
4. durable flush remains the fence before post-commit publication,
5. restart and replay still come from journal plus checkpoints, never from replaying ingress.

## Current Problem

The runtime still fuses three concerns that should be separate:

1. ingress transport,
2. worker ownership,
3. journal durability.

Today that coupling shows up as:

1. `AssignmentDelta`, `assigned_partitions`, and `world_is_assigned(...)` still gate worker
   activation and local followups,
2. `activate_assigned_worlds()` still loads worlds by journal/ingress partition implication,
3. broker ingress polling and offset-commit semantics still shape the worker core even when direct
   HTTP acceptance is the desired mode,
4. scheduler bookkeeping still carries partition-oriented concepts for external input progress,
5. there is no clean hosted story that says “this worker owns these worlds because config says so”.

This kept Kafka ingress alive as the conceptual center even when direct HTTP was the actual product
surface.

## Current Status

The implementation work for this phase is now substantially complete.

Landed:

1. hosted external submissions now enter through the direct runtime acceptance path,
2. `create_world`, event ingress, receipt ingress, and command ingress use one coherent
   acceptance model,
3. worker ownership is explicit runtime/config state via owned-world configuration,
4. assignment-driven activation and hosted Kafka ingress were removed from the runtime model,
5. optional `wait_for_flush` exists on the direct HTTP/control ingress path,
6. journal and checkpoint seams now exist as transport-neutral backend contracts in `aos-node`,
7. hosted rejects fake `latest_durable` read semantics and keeps reads hot/speculative,
8. Kafka may still be used as the journal backend, but only as a journal backend.

What remains is closeout work:

1. finish broader wait-path regression coverage,
2. keep the documentation/checklists aligned with the landed implementation.

## Design Stance

### 1) Preserve the hosted correctness invariants, not the old ingress model

The correctness invariants from `roadmap/v0.18-execution/architecture/hosted-architecture.md`
still stand and should survive this phase:

1. ingress is transient transport, not replay state,
2. restart comes from checkpoints plus journal, never from replaying ingress,
3. each loaded world has exactly one serialized kernel driver,
4. frames are durably appended before opened effects from those frames are published,
5. receipts, timer completions, and runtime continuations re-enter only as `WorldInput`,
6. runtime execution state remains reconstructible from kernel state plus substrate state,
7. the async runtime never writes the journal directly.

What should not survive as a general rule is this:

1. ingress partitions define worker ownership,
2. consumer-group assignment defines active-world eligibility,
3. offset advancement is the generic model of successful external acceptance.

Those were properties of the old Kafka-ingress architecture, not the target hosted architecture.

### 2) Accepted input is the seam

The worker should be organized around one accepted-input seam:

1. accept external input,
2. enqueue it into the scheduler,
3. service worlds,
4. stage completed slices,
5. flush them durably,
6. notify any waiter only after flush succeeds.

That seam is the architectural center for this phase.

Kafka ingress is not a peer producer of that seam anymore.

### 3) Ownership must be explicit and transport-independent

Hosted worker ownership should be represented as explicit runtime/config state, not inferred from
ingress-topic rebalance.

For P2, this can remain simple:

1. configured world inventory,
2. configured world IDs,
3. configured journal partitions for recovery if the current journal backend still needs them.

The important point is that ownership is no longer granted by ingress transport mechanics.

Later phases can make world discovery dynamic and pluggable.
P2 only needs to stop making ingress assignment the source of truth.

### 4) Direct HTTP/control is the only hosted ingress

The default hosted deployment should accept external submissions directly through the colocated
HTTP/control path.

That means:

1. `submit_event`,
2. `submit_receipt`,
3. `submit_command`,
4. `create_world`

should all enter the same direct acceptance path.

Kafka ingress is removed from hosted runtime architecture rather than kept as a parallel mode.

### 5) Optional wait-for-flush belongs on the acceptance path

Hosted should support an optional mode where an HTTP ingress call waits until the accepted item is
durably flushed.

This does **not** create a second read model.

The intended behavior is:

1. default mode returns once the item is accepted and enqueued,
2. optional wait mode blocks until the accepted item is durably flushed or times out,
3. failures are reported honestly from the acceptance/flush path,
4. reads remain hot/speculative unless and until a distinct durable read fence is designed.

### 6) Keep the current worker stage/flush machinery

This phase should keep the current hosted worker core shape:

1. per-world mailboxes,
2. completed speculative slices,
3. batched flush,
4. post-commit followups,
5. rollback/reopen on failed flush.

The goal is not to redesign the worker scheduler.
The goal is to remove Kafka-ingress ownership assumptions from around it.

### 7) Keep the slice and async execution models, change the backend seam around them

The current hosted execution model is still the right one even if the journal backend changes:

1. service one world deterministically,
2. collect the resulting durable records and opened effects into a speculative `CompletedSlice`,
3. stage that slice,
4. durably flush it,
5. only after durable flush do post-commit followups and async effect publication advance.

The async model is also still correct:

1. async executors never mutate world state directly,
2. opened effects are only started after durable flush,
3. receipts and other continuations re-enter only as `WorldInput`.

So P2 should preserve the slice model and async execution model, while changing the seam around
them.

The thing to replace is not the worker pipeline.
The thing to replace is the assumption that Kafka defines durable head, flush commit, replay
recovery, and ownership all at once.

### 8) Introduce a real backend seam around journal, checkpoints, and ownership

P2 should prepare for later switchable journals by separating three responsibilities:

1. journal append / durable flush / replay,
2. checkpoint persistence,
3. ownership and world inventory.

Those concerns should not be collapsed into one backend type and should not be defined by ingress
transport behavior.

The intended direction is:

1. worker/runtime owns staging, rollback, post-commit, waiter resolution, and async effect
   publication,
2. a journal backend provides durable append/flush plus enough durable-head information to assign
   correct `world_seq`,
3. checkpoint persistence remains a separate backend concern,
4. world ownership/inventory remains a runtime/discovery concern rather than a journal concern.

This keeps the hosted execution model stable while making the durable substrate replaceable.

Indicative seam sketch:

```rust
pub struct JournalFlush {
    pub frames: Vec<WorldLogFrame>,
    pub dispositions: Vec<DurableDispositionRecord>,
}

pub struct WorldDurableHead {
    pub next_world_seq: u64,
}

pub trait JournalBackend {
    fn durable_head(&self, world_id: WorldId) -> Result<WorldDurableHead, BackendError>;
    fn commit_flush(&mut self, flush: JournalFlush) -> Result<JournalCommit, BackendError>;
    fn world_frames(&self, world_id: WorldId) -> Result<Vec<WorldLogFrame>, BackendError>;
}

pub trait WorldCheckpointBackend {
    fn latest_checkpoint(&self, world_id: WorldId)
        -> Result<Option<WorldCheckpointRef>, BackendError>;
    fn commit_checkpoint(&mut self, checkpoint: WorldCheckpointCommit)
        -> Result<(), BackendError>;
}

pub trait WorldInventoryBackend {
    fn owned_worlds(&self) -> Result<Vec<WorldId>, BackendError>;
}
```

This example is intentionally indicative, not binding.
The important design point is the split:

1. hosted worker keeps slices, rollback, post-commit, and waiter resolution,
2. journal backend provides durable head plus durable flush,
3. checkpoint backend persists checkpoints,
4. inventory/ownership stays outside the journal backend.

### 9) Move only stable shared seams into `aos-node`

Higher-level public seams that are transport-neutral should live in `aos-node`.

This phase should move or define shared contracts for:

1. acceptance request/response semantics exposed through the HTTP API,
2. optional wait-until-durable parameters and result semantics,
3. transport-neutral acceptance token / flush-notification concepts if they are stable enough.

This phase should **not** move hosted runtime orchestration into `aos-node`.

Hosted-specific implementations should remain in `aos-node-hosted`, including:

1. scheduler message types,
2. direct acceptance queue implementation,
3. waiter registry implementation,
4. ownership/config activation plumbing,
5. journal flush batching and rollback details.

### 10) Journal partitioning may remain temporarily, but it is no longer ownership

If Kafka remains the journal backend for now, `partition_for_world(...)` can still define where
frames are durably appended.

That is a storage/ordering concern, not an ingress or ownership concern.

P2 should keep that boundary clear:

1. journal partitioning may remain backend-specific,
2. worker ownership becomes explicit runtime/config state,
3. ingress transport becomes direct HTTP/control only.

## Read Semantics In P2

P2 does not change the hosted read stance introduced in P1.

Reads remain:

1. hot,
2. in-process,
3. potentially speculative relative to durable flush.

P2 may add optional caller waiting on durable acceptance, but it does **not** turn normal hosted
reads into durable-replica reads.

As adjacent cleanup, hosted should stop advertising `latest_durable` as if it were a supported
read guarantee on the direct runtime path.

## Implementation Milestones

P2 should remain one architectural phase, but it should not be implemented as one giant code
change.

Recommended internal rollout:

### `P2.1` Direct Acceptance Only

Status: implemented.

1. remove hosted submission dependence on Kafka ingress,
2. make direct HTTP/control the only hosted external ingress path,
3. converge `create_world` toward the same acceptance path.

### `P2.2` Explicit Ownership

Status: implemented.

1. remove assignment-driven activation,
2. make owned-world config/runtime state the source of truth,
3. admit local continuations because the worker owns the world, not because an ingress partition is
   assigned.

### `P2.3` Backend Seam

Status: implemented.

1. introduce the journal/checkpoint/inventory split,
2. preserve `CompletedSlice`, rollback, post-commit, and async publication,
3. move durable-head and durable-flush behavior behind the journal seam.

### `P2.4` Wait-Until-Flushed And Cleanup

Status: mostly implemented; remaining work is test/doc closeout.

1. add optional HTTP wait semantics,
2. remove stale API/runtime language that implies the old model,
3. finish test coverage around direct-only ingress and explicit ownership.

## Scope

### [x] 1) Remove Kafka ingress from hosted runtime

Delete the hosted ingress-topic path and its worker-facing concepts.

Main intent:

1. remove `BrokerKafkaIngress` and ingress polling from hosted runtime flow,
2. remove consumer-group assignment as a driver of worker activation,
3. stop carrying ingress-topic submission as a first-class hosted execution mode.

### [x] 2) Make direct acceptance the only external hosted path

Direct HTTP/control acceptance should enqueue accepted items into the hosted worker without any
intermediate Kafka ingress write/read loop.

This direct path should be the default and only hosted ingress for:

1. `create_world`,
2. `submit_event`,
3. `submit_receipt`,
4. `submit_command`.

### [x] 3) Introduce explicit worker ownership/config activation

Replace ingress-assignment-based activation with explicit configured ownership.

For P2 this may remain static and runtime-local.

Main intent:

1. worlds owned by a worker are defined by config/runtime state,
2. local followups are admitted because the worker owns the world, not because an ingress partition
   is assigned,
3. recovery/bootstrap uses explicit owned inventory rather than ingress assignment callbacks.

### [x] 4) Keep accepted-input and flush acknowledgement transport-neutral

The worker core should continue using transport-neutral accepted-input and flush acknowledgement
concepts.

For P2:

1. direct HTTP-backed inputs carry accept tokens and optional waiter state,
2. local continuations remain separate from external acceptance,
3. no worker-facing type should require ingress-topic partition ownership semantics.

### [x] 5) Introduce a backend seam that preserves the current slice/async pipeline

Refactor the hosted runtime so journal-specific behavior sits behind a real backend seam without
changing the worker execution model.

Main intent:

1. keep `CompletedSlice`, staged flush, rollback, and post-commit behavior in hosted runtime,
2. move durable-head lookup and durable flush commit behind a journal backend seam,
3. keep checkpoint persistence separate from journal append semantics,
4. keep ownership and world inventory separate from the journal backend.

This is the seam that later phases can use for switchable journal backends without reworking the
worker model itself.

### [x] 6) Add optional HTTP wait-until-flushed behavior

Add an explicit API option for callers that want acceptance plus durable flush before the HTTP
request returns.

Recommended contract:

1. default: accepted immediately, return `accept_token`,
2. optional wait mode: block until the accepted item is durably flushed,
3. timeout/cancellation produce an honest partial result rather than a fake durable guarantee.

This option should live in the shared HTTP/API model, while the waiter implementation remains
hosted-specific.

### [x] 7) Converge `create_world` onto the same acceptance path

`create_world` should stop being the odd path with different acceptance behavior.

The same acceptance/wait semantics should apply to world creation as to other external inputs.

### [x] 8) Separate journal concerns from ownership concerns

Keep or adapt the current journal backend without letting it define runtime ownership.

Main intent:

1. journal append/replay remains durable-backend behavior,
2. ownership/config activation remains runtime behavior,
3. later journal-backend work is simplified because ingress has already been removed from the model.

### [~] 9) Update tests around direct acceptance and explicit ownership

Hosted tests should prove:

1. external submissions work with no Kafka ingress dependency,
2. worker activation follows explicit owned-world config,
3. flush/wait semantics are correct,
4. opened effects still publish only after durable append,
5. rollback/reopen behavior still works with the simplified ingress model,
6. the slice model and async publication model remain correct under the new backend seam.

Current state:

1. direct-only ingress, explicit ownership, rollback/retry, and hot-read behavior are covered,
2. at least one end-to-end `wait_for_flush` HTTP path is covered,
3. additional explicit wait-path coverage for `create_world`, receipt, command, and timeout
   behavior remains good closeout work but is not an architectural blocker.

## Non-Goals

P2 does **not** attempt:

1. full dynamic world discovery,
2. replacing `PartitionCheckpoint` with world checkpoints,
3. final switchable journal backends,
4. merging `aos-node-local` and `aos-node-hosted`,
5. redesigning the hosted scheduler core,
6. inventing a durable read fence,
7. preserving Kafka ingress as a supported hosted mode.

## Deliverables

1. A hosted runtime whose only external ingress path is direct HTTP/control acceptance.
2. A hosted worker activation model based on explicit ownership/config rather than ingress
   partition assignment.
3. A hosted worker acceptance seam organized around accepted input and durable flush.
4. An optional HTTP wait-until-flushed mode with explicit timeout/error semantics.
5. Shared acceptance/wait API contracts placed in `aos-node` where appropriate.
6. Hosted-specific ownership/orchestration logic kept in `aos-node-hosted`.

## Acceptance Criteria

1. Hosted external submissions no longer require or reference Kafka ingress.
2. Worker activation and local continuation admission no longer depend on ingress consumer-group
   assignment.
3. A caller can optionally wait for durable flush of an accepted HTTP submission.
4. No opened effect from a slice is published before that slice is durably flushed.
5. `create_world`, event ingress, receipt ingress, and command ingress all use one coherent
   acceptance model.
6. Shared seams added to `aos-node` are transport-neutral and do not pull hosted scheduler
   structure into the generic model layer.
7. Any remaining journal-partition logic is treated as backend storage behavior, not ownership or
   ingress behavior.
8. The slice model and async execution model are preserved rather than redesigned around the new
   backend seam.

Status:

All architectural acceptance criteria above are now satisfied in code.
Remaining work is limited to broader regression coverage and final roadmap closeout.

## Remaining Closeout

1. add explicit HTTP wait-path tests for `create_world`, receipt, command, and timeout behavior,
2. keep this roadmap entry aligned with the landed implementation as follow-on cleanup lands,
3. then mark P2 complete and move on to the next phase.
