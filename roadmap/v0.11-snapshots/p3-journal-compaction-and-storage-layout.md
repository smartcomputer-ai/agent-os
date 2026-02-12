# P3: Journal Compaction Plan (Segmented Hot Tail + Cold Segments)

**Priority**: P3
**Effort**: High
**Risk if deferred**: Medium (retention works but storage costs grow quickly)
**Status**: Proposed

## Goal

Define and implement the compaction model that works both locally and in future
distributed infra: keep a hot tail online, move older journal history to
segments, and preserve deterministic restore/audit behavior.

## Compaction Model

### 1. Segmented journal layout

Adopt explicit segment representation instead of a single monolithic log:

- hot segments: active replay path
- cold segments: compacted/exported history

Local FS example:

- `.aos/world/journal/hot/*.log`
- `.aos/world/journal/cold/*.log` (or archive dir)
- segment index metadata in world control state

Distributed target mapping (future):

- ordered metadata/index in control DB
- segment bodies in object store

### 2. Hot-tail + cold-segment semantics

- Journal remains authoritative append-only logically.
- Physical storage can move old ranges into segment blobs.
- Restore for full history mode:
  - baseline snapshot
  - cold segments covering retained range
  - hot tail to head

### 3. Compaction safety window

Compaction window `[h0..h1]` must satisfy:

- `h1 <= baseline.height - safety_margin` or equivalent policy
- receipt horizon safety checks pass
- segment index commit is atomic before source-range deletion

## Kernel/Protocol Hooks

- Add journal segment index API for deterministic scanning.
- Ensure replay can merge segment sources without behavioral divergence.
- Add compaction marker events/records for auditability.

## CLI/Ops

- `aos journal compact plan`
- `aos journal compact run`
- `aos journal verify`

`verify` checks:

- contiguous height coverage
- checksum/hash integrity
- ability to replay across baseline + segments + tail

## Tests

- Compaction preserves byte-identical replay state.
- Crash-in-middle cases: no history loss on restart.
- Segment index corruption detection.

## DoD

- Local segmented compaction is implemented and replay-safe.
- Retention can target segments rather than raw entries.
- Design is explicitly compatible with future infra storage split
  (metadata DB + object store segments).

## Non-Goals

- Multi-world scheduler/worker leasing.
- Shared-CAS distributed GC.
- Universe messaging transport.
