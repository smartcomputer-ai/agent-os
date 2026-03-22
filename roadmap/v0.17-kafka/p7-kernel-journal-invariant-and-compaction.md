# P7: Kernel Journal Invariant and In-Place Compaction

**Priority**: P7  
**Effort**: High  
**Risk if deferred**: High (the system will keep multiple journal-shaped seams alive and hot worlds will have no clean bounded-history model)  
**Status**: Completed

## Goal

Make the kernel journal a universal runtime invariant while keeping node/runtime durability policy
above the kernel.

Do this in one cutover:

- remove the current split between `MemJournal`, fs journal, and sqlite journal
- remove journal backend mode as an architectural choice
- land one unified kernel journal implementation
- make every hot world rely on that one implementation
- support bounded retained history after checkpoint-driven compaction
- do not reopen worlds just because a snapshot/checkpoint was written

## Completed In Code

Implemented on the experimental branch:

1. `crates/aos-kernel/src/journal.rs` now defines a single concrete `Journal` type.
2. The old kernel journal split has been removed:
   - `MemJournal`
   - fs journal
   - `pub trait Journal`
3. The old local raw sqlite journal path has been removed.
4. Kernel, runtime, node, hosted-worker, and test code now use the unified `Journal`.
5. Replay seeding now happens through `Journal::from_entries(...)` rather than backend-specific
   journal implementations.
6. The unified journal now exposes retained-bounds and in-place prefix compaction primitives.
7. Tooling such as `aos-smoke` now runs on the unified kernel journal without waiting for broader
   persisted-local unification.
8. The local node runtime now compacts the hot kernel journal in place after persisting a new
   checkpoint head.
9. Local journal read APIs now report retained bounds explicitly and no longer assume retained
   length equals absolute journal sequence.
10. The hosted node runtime now compacts the hot kernel journal in place after durable checkpoint
    commit succeeds.
11. Hosted worker tail capture now uses absolute journal bounds rather than retained journal
    length.
12. The shared `aos-node` log-first runtime now uses absolute journal bounds for tail capture and
    compacts hot worlds after checkpoint publication.
13. Shared trace/introspection surfaces now expose retained journal bounds explicitly.
14. Tests covering local, hosted, shared log-first, and runtime trace flows now assert the
    retained-prefix model.

Follow-on work outside `P7`:

1. Broader product/runtime checkpoint orchestration still belongs to later node-layer work.

## Problem Statement

Today the codebase still carries multiple journal-shaped paths:

- `pub trait Journal: Send`
- helper disk encoding/decoding in the journal module
- kernel `MemJournal`
- filesystem journal through `with_fs_journal`
- local sqlite journal through `LocalSqliteJournal`
- node/runtime frame logs that later reconstruct `OwnedJournalEntry`

This should not survive as transition code.

The directive for this phase is:

1. delete the multiple journal implementations
2. replace them with one kernel journal implementation
3. update callers to use that implementation everywhere
4. add in-place compaction semantics to that implementation

## Directive

The kernel always has a journal.

There should be one journal module and one journal implementation.

Delete:

- `pub trait Journal`
- `MemJournal`
- fs journal
- `with_fs_journal`
- `LocalSqliteJournal` as a journal architecture

Do not preserve them as supported variants.

The end state of this phase is:

- one journal concept
- one implementation
- one compaction model
- one concrete `Journal` type in the kernel

At the same time:

- the kernel journal is not where route metadata lives
- the kernel journal is not where submission transport semantics live
- the kernel journal is not where partition checkpoint manifests live

Those remain node/runtime concerns.

## Journal Model

The journal remains the kernel's canonical ordered record stream.

It is responsible for recording:

- domain events
- effect intents
- effect receipts
- stream frames
- manifest changes
- governance records
- snapshot records
- capability and policy decisions
- custom runtime records

But the journal must stop implying "full retained history from seq 0 forever".

Instead it should support:

- absolute monotonic sequence numbers
- retained-prefix tracking
- bounded batch reads
- safe in-place prefix compaction after checkpoint durability

## Snapshot vs Checkpoint

These should remain distinct.

### Snapshot

A snapshot is a kernel-level state image at journal height `H`.

It is created by the kernel and represented in canonical history by a `Snapshot` journal record.

### Checkpoint

A checkpoint is a node/runtime-level statement that a snapshot is now the durable restart baseline
for the surrounding runtime.

It may carry additional metadata outside the kernel such as:

- route/epoch context
- retained-journal bounds
- partition progress
- world-directory state

Compaction should be driven by checkpoint durability, not merely by snapshot creation.

## Required Journal Capabilities

The old journal API shape was wrong for the desired steady state.

The replacement should be a concrete `Journal` type with semantics like:

- append one or more entries
- read retained entries from an absolute sequence
- report retained lower bound
- report next sequence
- compact through an acknowledged sequence boundary

Representative shape:

```rust
pub struct JournalBounds {
    pub retained_from: u64,
    pub next_seq: u64,
}

pub struct Journal {
    // internal retained storage + compaction metadata
}

impl Journal {
    pub fn new() -> Self;
    pub fn from_entries(entries: &[OwnedJournalEntry]) -> Result<Self, JournalError>;
    pub fn append(&mut self, entry: JournalEntry<'_>) -> Result<JournalSeq, JournalError>;
    pub fn append_batch(
        &mut self,
        entries: &[JournalEntry<'_>],
    ) -> Result<JournalSeq, JournalError>;
    pub fn load_batch_from(
        &self,
        from: JournalSeq,
        limit: usize,
    ) -> Result<Vec<OwnedJournalEntry>, JournalError>;
    pub fn bounds(&self) -> JournalBounds;
    pub fn compact_through(
        &mut self,
        inclusive_seq: JournalSeq,
    ) -> Result<(), JournalError>;
}
```

These semantics now exist in the unified implementation.

Completed removals from the API surface:

- trait-based journal polymorphism
- `set_next_seq`
- `next_seq()` as the only bounds primitive
- backend-specific construction in `KernelBuilder`

Expected retained core types:

- `JournalSeq`
- `JournalKind`
- `JournalRecord`
- `JournalEntry`
- `OwnedJournalEntry`
- `JournalError`

## In-Place Compaction Requirement

Hot worlds must not be forced to reopen just because they reached a new checkpoint boundary.

That would:

- reinitialize WASM modules unnecessarily
- throw away hot caches
- make compaction a disruptive lifecycle event

Required stance:

1. the world keeps running hot
2. the kernel emits a snapshot at height `H`
3. the node/runtime persists the durable checkpoint around that snapshot
4. once durability is confirmed, the hot journal compacts its retained prefix through `H`
5. the world continues without reinitialization

This means all journal-reading code must tolerate:

- retained history starting at `retained_from > 0`
- full-history reads meaning "full retained history", not "since genesis"

## Recovery Model

Cold restart still works through:

- durable checkpoint baseline
- retained authoritative history after that baseline

Warm in-process compaction is different:

- it does not reopen the world
- it does not rebuild host state
- it only shortens the retained journal prefix once that prefix is safely subsumed by checkpointed
  state

## Relationship To Node-Level Durability

This phase does not decide that raw kernel journal rows become the only node-level truth.

It does decide:

- kernel code should stop having multiple journal implementations immediately
- journal lifecycle and retention should become explicit and correct
- the node/runtime remains responsible for durability policy and checkpoint publication

In other words:

- kernel owns journal semantics
- node owns checkpoint and durability policy

## Local And Hosted Consequences

### Local

The embedded local runtime should run hot worlds against the same kernel journal invariant and
compact them in place after durable checkpoint advancement.

Important near-term consequence:

- local developer/test tooling that does not require durable persisted-local behavior yet should
  still cut over immediately to the unified kernel journal
- for example, `aos-smoke` should continue working on the new kernel journal even before broader
  persistence/path unification lands
- lack of immediate persistence integration is not a reason to preserve old journal
  implementations

### Hosted

Hosted workers should also treat the kernel journal as the hot execution log inside a partition
owner, while keeping Kafka/object-store routing, publication, and recovery policy at the node
layer.

This phase does not require hosted to collapse Kafka transport directly into the kernel journal
API.

## Repository Consequences

Required repository changes:

- replace the current journal submodule family with one unified journal implementation
- make `crates/aos-kernel/src/journal/mod.rs` define the concrete `Journal`
- remove `mem.rs`
- remove `fs.rs`
- delete `MemJournal`
- delete fs journal
- delete `with_fs_journal`
- delete `LocalSqliteJournal` as a journal backend path
- remove `with_journal(...)` as a general backend injection seam
- remove `set_journal_next_seq(...)`
- update runtime, harness, local, and test code to use the unified kernel journal
- remove journal backend mode as a meaningful distinction from docs and code

Remaining repository work under this phase is narrower:

- wire checkpoint advancement to `Journal::compact_through(...)`
- make retained-prefix semantics explicit wherever the code still assumes "full history since 0"

## Out of Scope

1. Moving route, submission, or partition-checkpoint concerns into the kernel.
2. Declaring raw durable kernel journal rows to be the one node-level truth source.
3. Requiring every checkpoint write to rebuild or reopen the world.
4. Solving all query/projection retention policy in this phase.
5. Preserving old journal implementations for compatibility.

## DoD

1. The roadmap states that every hot kernel runs with a journal as an invariant.
2. The roadmap distinguishes snapshot creation from checkpoint durability.
3. The roadmap requires in-place journal compaction after checkpoint durability.
4. The roadmap explicitly rejects forced world reopen just to compact journal history.
5. The roadmap keeps node/runtime durability policy above the kernel.
6. The roadmap eliminates journal backend mode as an architectural product distinction.
7. `MemJournal`, fs journal, and sqlite journal are all called out for removal rather than
   preservation.
8. The roadmap states plainly that this cutover should be done in one go.
9. The roadmap states plainly that the end state is a concrete `Journal` type rather than a
   backend trait with multiple implementations.
10. The roadmap states that tooling such as `aos-smoke` should move to the unified kernel journal
    immediately even if persisted-local unification lands later.
