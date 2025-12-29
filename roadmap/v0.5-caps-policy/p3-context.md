# p3-context: Deterministic Call Context ("bowl")

## TL;DR
Reducers and pure modules may receive a small, deterministic **call context** when they
declare it in the module ABI. Reducers get time/entropy/journal metadata; pure modules
get a minimal context (logical time + journal height + manifest hash).
The kernel samples wall clock and monotonic time **at ingress**, journals them, and replays
those values during execution.

This keeps reducers isolated (no implicit world reads) while making time/entropy ergonomic
and replayable, similar to Urbit's bowl but without global state access.

---

## Goals
- Deterministic and replayable across shadow runs and replays.
- Provide time/entropy/journal metadata without turning them into effects (when modules
  opt in).
- Keep reducers isolated; cross-reducer reads remain plan-only (`introspect.*`).
- Versioned context schema to allow future expansion.

## Non-Goals
- Synchronous world-state reads inside reducers.
- Kernel using host wall clock for authorization or policy decisions.
- Unbounded or opaque context maps.

---

## Context Inputs (Deterministic Sources)

The kernel captures the following **at ingress** and records them on every
`DomainEventRecord` / `EffectReceiptRecord`:

- **now_ns**: host wall clock at ingress, stored and replayed verbatim.
- **logical_now_ns**: monotonic kernel time at ingress, stored and replayed verbatim.
- **entropy**: host RNG bytes captured at ingress, stored and replayed verbatim.
- **journal_height**: the sequence number assigned to the entry.
- **event_hash**: sha256 of the canonical event envelope (schema + value + key).
- **manifest_hash**: manifest hash pinned for this world/entry.

These become the call context fields seen by reducers (and a reduced subset for pures)
when the module declares a context schema.

---

## Schemas

The reducer and pure contexts are **distinct** schemas to keep surfaces minimal.

### `sys/ReducerContext@1`

Built-in schema (canonical CBOR on the wire):

```
{
  now_ns: nat,                // wall clock at ingress
  logical_now_ns: nat,        // monotonic kernel time
  journal_height: nat,
  entropy: bytes,             // REQUIRED length: 64 bytes
  event_hash: hash,
  manifest_hash: hash,

  // Invocation metadata
  reducer: text,              // Name-formatted string
  key: option bytes,          // cell key, if any
  cell_mode: bool             // keyed reducer routing
}
```

Notes:
- `now_ns` is informational for reducer logic. It must **not** be used by kernel
  authorization logic; caps/policy continue to use `logical_now_ns`.
- `entropy` size is fixed for stability; tests can use deterministic RNG seeded by fixtures.
- `cell_mode` retains v1 compatibility semantics where `key` can be advisory.
- The timer system should source time from the kernel clock (privileged adapter or
  kernel-assisted timers).
- `event_hash` is sha256 of the canonical DomainEvent delivered to the reducer. It is
  intended for deterministic correlation/idempotency without re-hashing the event bytes.

### `sys/PureContext@1`

Built-in schema (canonical CBOR on the wire):

```
{
  logical_now_ns: nat,        // monotonic kernel time
  journal_height: nat,
  manifest_hash: hash,
  module: text                // Name-formatted string
}
```

Pure modules do not receive `now_ns`, `entropy`, or `event_hash` by default.

---

## ABI Changes

### Reducer input envelope

Previous (pre-context):
```
{ version: 1, state: <bytes|null>, event: <bytes> }
```

Current (context optional):
```
{
  version: 1,
  state: <bytes|null>,
  event: <bytes>,
  ctx: <bytes>   // canonical CBOR for sys/ReducerContext@1 (omitted if not declared)
}
```

### Pure module input envelope

Previous (pre-context):
```
{ version: 1, input: <bytes> }
```

Current (context optional):
```
{
  version: 1,
  input: <bytes>,
  ctx: <bytes>   // canonical CBOR for sys/PureContext@1 (omitted if not declared)
}
```

---

## Module ABI Declaration

Reducers and pure modules **may** declare their expected context schema:

```
abi: {
  reducer: {
    state: "...",
    event: "...",
    context: "sys/ReducerContext@1",
    effects_emitted: [...],
    cap_slots: {...}
  }
}
```

Pure module example:

```
abi: {
  pure: {
    input: "...",
    output: "...",
    context: "sys/PureContext@1"
  }
}
```

If omitted, the kernel does **not** send a context envelope to that module.
Modules that need context (time, key, etc.) should declare it explicitly.

---

## Determinism and Governance

- Context values are **journaled**; replay uses the recorded values, not live host data.
- `now_ns` is not used for policy/cap decisions. `logical_now_ns` remains the only
  time source for expiry and policy windows.
- `logical_now_ns` is sourced from the kernel's monotonic clock at ingress; receipt
  timestamps do not drive it.
- `introspect.*` remains the plan-level "scry" equivalent for cross-reducer reads.
- Pure module context is derived from the **current ingress stamp** (same as the
  enclosing event/receipt); no separate journal entry is created when context is declared.

---

## Implementation Notes (As Built)

- Context schemas are now built-ins: `sys/ReducerContext@1` and `sys/PureContext@1`.
- `defmodule` makes `context` optional for both reducer and pure ABIs.
- Kernel stamps ingress with `now_ns`, `logical_now_ns`, `entropy`, `journal_height`,
  `event_hash` (canonical DomainEvent CBOR), and `manifest_hash`.
- Context bytes are only sent when a module declares `context`; otherwise `ctx` is omitted.
- Modules that require `ctx.key()` must declare reducer context (e.g., keyed reducers).
- The reducer/pure ABI envelopes now carry `ctx` as canonical CBOR bytes (optional).
- WASM build cache includes path dependency hashes so SDK/ABI changes trigger rebuilds.

---

## v0.5 Target

- Add `sys/ReducerContext@1` and `sys/PureContext@1` to `spec/defs/builtin-schemas.air.json`.
- Extend reducer and pure module ABI envelopes to carry optional context bytes.
- Populate context from journal entry metadata at invocation time.
- Keep caps/policy time based on `logical_now_ns` (no change in authorizer semantics).
- Validate `entropy` length (64 bytes) at load/validation time.
- Omit context envelopes for modules that do not declare a context schema.
