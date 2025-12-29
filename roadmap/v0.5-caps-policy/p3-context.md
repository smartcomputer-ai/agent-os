# p3-context: Deterministic Call Context ("bowl")

## TL;DR
Reducers and pure modules should receive a small, deterministic **call context** on every
invocation. The context includes ingest time, entropy, journal height, and identity metadata.
The kernel never reads wall clock during execution; instead it **stamps events/receipts at
ingress** and replays those values from the journal.

This keeps reducers isolated (no implicit world reads) while making time/entropy ergonomic
and replayable, similar to Urbit's bowl but without global state access.

---

## Goals
- Deterministic and replayable across shadow runs and replays.
- Provide time/entropy/journal metadata without turning them into effects.
- Keep reducers isolated; cross-reducer reads remain plan-only (`introspect.*`).
- Versioned context schema to allow future expansion.

## Non-Goals
- Synchronous world-state reads inside reducers.
- Kernel using host wall clock for authorization or policy decisions.
- Unbounded or opaque context maps.

---

## Context Inputs (Deterministic Sources)

The kernel captures the following **at ingress** (event submission or receipt injection)
and records them in the journal entry:

- **ingest_time_ns**: host wall clock at ingress, stored and replayed verbatim.
- **logical_now_ns**: monotonic kernel time at ingress, stored and replayed verbatim.
- **entropy**: host RNG bytes captured at ingress, stored and replayed verbatim.
- **journal_height**: the sequence number assigned to the entry.
- **event_hash**: sha256 of the canonical event envelope (schema + value + key).
- **manifest_hash**: manifest hash pinned for this world/entry.

These become the call context fields seen by reducers and pure modules.

---

## Schema: `sys/CallContext@1`

Proposed built-in schema (canonical CBOR on the wire):

```
{
  now_ns: nat,                // ingest_time_ns (wall clock)
  logical_now_ns: nat,        // monotonic kernel time
  journal_height: nat,
  entropy: bytes,             // REQUIRED length: 64 bytes
  event_hash: hash,
  manifest_hash: hash,

  // Invocation metadata
  caller_kind: text,          // "reducer" | "pure"
  caller_name: text,          // Name-formatted string
  key: option bytes,          // cell key, if any
  cell_mode: bool,            // keyed reducer routing
  cell_act: nat               // per-cell event counter (0 for unkeyed)
}
```

Notes:
- `now_ns` is informational for reducer/pure logic. It must **not** be used by kernel
  authorization logic; caps/policy continue to use `logical_now_ns`.
- `entropy` size is fixed for stability; tests can use deterministic RNG seeded by fixtures.
- `cell_mode` retains v1 compatibility semantics where `key` can be advisory.
- The timer system should source time from the kernel clock (privileged adapter or
  kernel-assisted timers).

If a field is not applicable (e.g., pure module invocation), it must be set to a well-defined
default (empty key, `cell_mode=false`, `cell_act=0`, and concrete caller identifiers).

---

## ABI Changes

### Reducer input envelope

Current:
```
{ version: 1, state: <bytes|null>, event: <bytes>, ctx: { key?, cell_mode } }
```

Proposed:
```
{
  version: 1,
  state: <bytes|null>,
  event: <bytes>,
  ctx: <bytes>   // canonical CBOR for sys/CallContext@1
}
```

### Pure module input envelope

Current:
```
{ version: 1, input: <bytes> }
```

Proposed:
```
{
  version: 1,
  input: <bytes>,
  ctx: <bytes>   // canonical CBOR for sys/CallContext@1
}
```

---

## Module ABI Declaration

Allow reducers and pure modules to declare their expected context schema:

```
abi: {
  reducer: {
    state: "...",
    event: "...",
    context: "sys/CallContext@1",
    effects_emitted: [...],
    cap_slots: {...}
  }
}
```

If omitted, the runtime defaults to `sys/CallContext@1`.

---

## Determinism and Governance

- Context values are **journaled**; replay uses the recorded values, not live host data.
- `now_ns` is not used for policy/cap decisions. `logical_now_ns` remains the only
  time source for expiry and policy windows.
- `logical_now_ns` is sourced from the kernel's monotonic clock at ingress; receipt
  timestamps do not drive it.
- `introspect.*` remains the plan-level "scry" equivalent for cross-reducer reads.

---

## v0.5 Target

- Add `sys/CallContext@1` to `spec/defs/builtin-schemas.air.json`.
- Extend reducer and pure module ABI envelopes to carry context bytes.
- Populate context from journal entry metadata at invocation time.
- Keep caps/policy time based on `logical_now_ns` (no change in authorizer semantics).
