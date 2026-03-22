# P20: Cleanup

**Priority**: P20  
**Effort**: Medium  
**Risk if deferred**: Low (the system can keep moving, but dead abstractions and leftover staging
logic make the worker harder to reason about than necessary)  
**Status**: In Progress

## Goal

Capture post-cutover cleanup work that is no longer on the critical path for `v0.17-kafka`, but is
worth revisiting once the main hosted architecture changes have landed.

This item is intentionally for cleanup and simplification, not for new product surface.

## Progress Snapshot

Completed:

- item 1: the old `SpeculativeWorld` create staging path has been replaced with narrower
  commit-aligned pending-create staging
- item 3: hosted checkpoint metadata write/reopen invariants have been repaired and covered by
  regression tests

Remaining:

- item 2: worker-internal cleanup and hardening follow-on
- durable command execution coverage on the worker path remains tracked here rather than in the
  completed P12 test phase

## Initial Candidates

### 1) Replace `SpeculativeWorld` With Create-Only Pending Staging (DONE)

Status: done.

Implemented outcome:

- the old `SpeculativeWorld` / `SpeculativeWorlds` create staging abstraction has been removed
- the create path now uses a narrower pending-created-world staging value
- pending create-world state remains batch-local until Kafka commit succeeds
- create-world finalization now inserts registered + active world state together after commit
- the old `promote_speculative_worlds(...)` shape has been collapsed into create finalization

Why this was worth doing:

- the old name suggested a broader speculative execution model than the code actually used
- the abstraction had become create-only and was carrying unnecessary shape from an older worker
  design
- the cleanup also removed the pre-commit registered-world mutation that could leave ghost
  registered state behind after an injected batch-commit failure

Regression coverage added:

- embedded create-batch commit failure now proves the worker retries cleanly without leaving
  registered ghost state behind

### 2) Trim Leftover Worker-Internal Complexity

Other cleanup candidates likely belong here as the hosted worker settles:

- narrow worker-only helper surfaces that were kept for expediency
- remove dead compatibility paths after the new projection/materializer model stabilizes
- simplify state transitions that still reflect older replay- or bootstrap-era assumptions
- add durable command execution coverage on the worker path without expanding P12 back into a
  larger catch-all hosted test phase

For that command-path follow-on, the useful target remains:

- queue a governance/admin command
- execute it on the real broker-backed worker path
- persist the final command-record state durably in blobstore
- prove restart/reread observes that final command state

### 3) Fix Hosted Checkpoint Metadata Merge On Restart (DONE)

Status: done.

Historical problem statement:

- hosted worker checkpoints are partition-scoped metadata objects that contain:
  - a partition `journal_offset`
  - a set of `WorldCheckpointRef` entries, one per world
- on checkpoint publish, the worker currently seeds the new checkpoint from the previously stored
  partition checkpoint for the same `universe_id + partition`
- that merge carries forward both:
  - old world entries
  - the old partition `journal_offset`
- on reopen, the worker restores a world's baseline snapshot and then replays only frames with
  `entry.offset > checkpoint.journal_offset`

Original issue:

- this is not a kernel snapshot corruption bug
- the failure is a checkpoint metadata consistency bug
- a fresh world can inherit a stale partition replay cut from blobstore checkpoint metadata even
  when its own `world_id` is new
- if Kafka has been reset or truncated, or if old checkpoint metadata from unrelated historical
  worlds is still present, the merged checkpoint can become internally inconsistent:
  - world baseline says "resume from height H"
  - partition `journal_offset` says "tail starts much later"
  - reopen then skips required frames between `H + 1` and the first frame after
    `checkpoint.journal_offset`

Observed repro:

- managed repro in `hosted-prof --scenario demiurge-restart-inproc` shows:
  - full frame replay succeeds
  - restoring the checkpoint baseline alone matches full replay at the same baseline
  - checkpoint reopen fails because the tail cut is wrong
- concrete example from repro:
  - baseline height = `3`
  - expected replay tail = world seq `4..171`
  - checkpoint reopen tail = world seq `18..171`
  - records `4..17` were silently skipped
- the same repro also showed `checkpoint_worlds=14` for a brand new world, which confirms that old
  blobstore checkpoint entries were being merged forward into the new partition checkpoint

Why it happened:

- the worker had enough normal runtime data to write a valid checkpoint, but chose the wrong source
  of truth for the partition replay watermark
- the checkpoint writer treated prior blobstore checkpoint metadata as authoritative and merged it
  into a fresh checkpoint without proving it still matched the currently recovered Kafka partition
- different `world_id`s do not protect against this because the bad replay cut is partition-scoped,
  not world-scoped

What was required:

- stop treating prior blobstore checkpoint metadata as blindly reusable input
- ensure a newly written checkpoint is derived from currently recovered partition state, not from a
  stale historical partition watermark
- guarantee this invariant:
  - for every `WorldCheckpointRef` stored in a partition checkpoint, the replay tail selected by
    that checkpoint must include all records needed to advance from that world's baseline

Realistic implementation options:

- Preferred: derive the checkpoint replay watermark from the worlds actually checkpointed in the
  current publish pass, not from the previous blobstore checkpoint.
  - the worker already knows, for each world it checkpoints:
    - the new baseline height
    - the just-appended checkpoint frame
    - the current recovered partition log entries
  - use only that live data to compute the replay cut
  - do not carry forward old partition `journal_offset` from blobstore

- Also required: stop carrying forward checkpoint entries for worlds that are not known-valid in
  the current recovered partition view.
  - only include worlds that the worker can currently account for from Kafka recovery plus current
    in-memory registration
  - if a prior checkpoint refers to worlds that are absent from the current partition log, drop
    them instead of preserving them

- Stronger structural fix: replace the partition-wide replay cut with per-world replay metadata.
  - this avoids one world's history influencing another world's reopen tail
  - this is a somewhat larger shape change, so it may not be the first repair, but it is the
    cleanest long-term design

- Add a safety validation before committing any checkpoint:
  - for each world entry in the checkpoint, verify that the chosen replay cut does not skip over
    that world's required post-baseline frames in the currently recovered partition log
  - if the validation fails, reject the checkpoint publish rather than persisting inconsistent
    metadata

Regression coverage that was needed:

- add a focused broker/blobstore test that constructs the stale-merge condition in a single test
  run without relying on pre-corrupted external state
- outline:
  1. use a fresh test universe id
  2. start a broker-backed hosted worker runtime with object-store-backed blob meta
  3. seed blobstore checkpoint metadata for the target partition with:
     - an old checkpoint containing unrelated worlds
     - a stale, advanced partition `journal_offset`
  4. create a brand new world in Kafka and let the worker publish its create-time checkpoint
  5. restart the worker
  6. assert that reopen either:
     - remains healthy and replays the full required tail after the fix
     - or, on current broken behavior, skips early frames and fails deterministically
- this test does not require the blobstore to already be "bad" from prior manual runs; it creates
  the exact stale checkpoint input explicitly as part of the setup

Resolved outcome:

- checkpoint metadata is self-consistent at write time
- restart recovery never skips required world frames because of stale partition-level offsets
- Kafka reset plus fresh world creation cannot inherit blobstore checkpoint watermarks from older
  worlds in the same universe/partition
- the regression is covered by a deterministic automated test

## Non-Goals

This item is not for:

- changing the authoritative journal model
- redesigning hosted query projections again
- introducing a new feature family

## DoD

P20 is complete when:

1. obviously stale worker/materializer cleanup candidates have been either removed or explicitly
   rejected with a clear rationale
2. the create-world staging path is simpler or more accurately named than it is today
3. hosted checkpoint metadata write/reopen invariants are explicit and covered by regression tests
4. the hosted worker execution path is easier to read without changing the core P11/P17 design
