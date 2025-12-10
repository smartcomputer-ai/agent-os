# P4: Introspection Surface Prerequisites

**Status:** Required foundations for `p4-worldfs-cli` and self-upgrade agents. This doc enumerates what must exist in the platform (effects, caps, control verbs, CLI/SDK wiring) before we can ship the WorldFS UX.

WorldFS is purely a veneer over introspection + catalog + CAS. Right now those primitives are partial or only exposed in-process. We need to standardize and capability-gate them so both CLI callers and in-world plans can read world state with consistency metadata.

---

## Gaps Today
- No `defeffect` entries for introspection (`introspect.manifest`, `introspect.reducer_state`, `introspect.journal_head`, optionally `introspect.list_cells`); only in-process `StateReader`.
- No `query` capability type or policy hooks for read-only introspection; reads are effectively ambient.
- Control socket lacks verbs for manifest/readers; CLI reads directly from `air/` or store without hashes/consistency.
- No blob **get** surface in control/CLI (only `put-blob`), so `/blob/**` and `/obj/*/data` cannot be wired.
- SDK/LLM helpers missing (`fs_read_manifest`, `fs_read_reducer`, `fs_ls`) so plans have to hand-roll effect calls.

---

## Required Work

### 1) Define introspection effects (+ schemas)
Add `defeffect`s under `spec/defs` (and bake into builtins):
- `introspect.manifest` → params `{ consistency: Head|Exact|AtLeast? }`, receipt `{ manifest, journal_height, snapshot_hash?, manifest_hash }`
- `introspect.reducer_state` → params `{ reducer: Name, key?: bytes, consistency }`, receipt `{ state_b64?, meta { journal_height, snapshot_hash?, manifest_hash } }`
- `introspect.journal_head` → params `{}` , receipt `{ journal_height, snapshot_hash?, manifest_hash }`
- Optional: `introspect.list_cells` → params `{ reducer: Name }`, receipt `{ cells: [{ key_b64, state_hash_hex, size, last_active_ns }] }`
All are read-only; receipts carry consistency metadata for downstream governance proposals.

### 2) Capabilities + policy
- Introduce `defcap` for query (e.g., `sys/query@1`, cap_type `query`) and wire policy matching on `effect_kind` prefix `introspect.*`; default deny.
- Reuse `blob` cap for CAS reads; ObjectCatalog read paths may also require `query`.
- Ensure built-in world manifests include a `query` cap grant for trusted operators/agents.

### 3) Kernel/adapter implementation
- Add an adapter/handler that routes `introspect.*` effects to `StateReader` (`Kernel::get_manifest`, `get_reducer_state`, `get_journal_head`, `list_cells`), returning canonical receipts with hashes/heights.
- Ensure deterministic replay consumes receipts (no re-running reads).

### 4) Control surface
- Extend control protocol with verbs that call the introspection adapter, not direct file reads:
  - `manifest-read` / `query-state` / `list-cells` / `journal-head` (reuse existing names where possible, but return consistency metadata).
  - Add `blob-get` verb returning base64 CAS bytes (needed for `/blob/**` and object payloads).

### 5) CLI/SDK/LLM helpers
- CLI commands (`world state`, `world manifest`, new `world fs`) must call control verbs first, batch fallback second, always emitting consistency metadata.
- Add SDK/LLM helpers `fs_read_manifest`, `fs_read_reducer`, `fs_ls`, `fs_stat`, `fs_read` that translate to `introspect.*` + `blob.get`.

### 6) Tests/fixtures
- Integration tests around `introspect.*` effects: head/exact/at-least, keyed reducers, cold snapshot reads, policy deny.
- Control/CLI tests: manifest/state read returns hashes and correct base64 decoding; blob-get round-trips.

---

## Dependencies / Tie-ins
- **p4-worldfs-cli**: blocked until introspection effects + control verbs exist; CLI should be layered on these.
- **p1-self-upgrade (v0.5)**: governance proposals need the consistency metadata from introspection receipts to attest what was read when preparing patches.

---

## Draft AIR Definitions (sketch)

### Query Capability
```jsonc
{
  "$kind": "defcap",
  "name": "sys/query@1",
  "cap_type": "query",
  "schema": { "record": { "scope": { "text": {}, "$comment": "Optional scope string; empty = all" } } },
  "$comment": "Grants use of introspect.* effects; policy can further restrict by reducer/name."
}
```

### Introspection Effects
```jsonc
[
  {
    "$kind": "defeffect",
    "name": "introspect.manifest",
    "params": { "record": { "consistency": { "text": {}, "$comment": "head | exact:<h> | at_least:<h>" } } },
    "receipt": { "record": {
      "manifest": { "ref": "air/Manifest@1" },
      "journal_height": { "nat": {} },
      "snapshot_hash": { "maybe": { "hash": {} } },
      "manifest_hash": { "hash": {} }
    }},
    "origin_scope": "plan-only",
    "cap_type": "query"
  },
  {
    "$kind": "defeffect",
    "name": "introspect.reducer_state",
    "params": { "record": {
      "reducer": { "text": {} },
      "key_b64": { "maybe": { "text": {} } },
      "consistency": { "text": {} }
    }},
    "receipt": { "record": {
      "state_b64": { "maybe": { "text": {} } },
      "meta": { "record": {
        "journal_height": { "nat": {} },
        "snapshot_hash": { "maybe": { "hash": {} } },
        "manifest_hash": { "hash": {} }
      }}
    }},
    "origin_scope": "plan-only",
    "cap_type": "query"
  },
  {
    "$kind": "defeffect",
    "name": "introspect.journal_head",
    "params": { "record": {} },
    "receipt": { "record": {
      "journal_height": { "nat": {} },
      "snapshot_hash": { "maybe": { "hash": {} } },
      "manifest_hash": { "hash": {} }
    }},
    "origin_scope": "plan-only",
    "cap_type": "query"
  },
  {
    "$kind": "defeffect",
    "name": "introspect.list_cells",
    "params": { "record": { "reducer": { "text": {} } } },
    "receipt": { "record": {
      "cells": { "list": { "record": {
        "key_b64": { "text": {} },
        "state_hash": { "hash": {} },
        "size": { "nat": {} },
        "last_active_ns": { "nat": {} }
      } } },
      "meta": { "record": {
        "journal_height": { "nat": {} },
        "snapshot_hash": { "maybe": { "hash": {} } },
        "manifest_hash": { "hash": {} }
      }}
    }},
    "origin_scope": "plan-only",
    "cap_type": "query"
  }
]
```

Notes:
- `consistency` is a textual envelope to keep params simple; runtime parses to `Head | Exact(u64) | AtLeast(u64)`.
- Receipts always include consistency metadata even when the payload is empty (e.g., missing reducer key).
- `cap_type` references the new `query` cap; blob reads still require `blob` cap when the CLI/SDK chains to `blob.get`.

---

## Control Protocol Additions (sketch)

- `manifest-read { consistency?: "head"|"exact:<h>"|"at_least:<h>" }` → `{ manifest, journal_height, snapshot_hash?, manifest_hash }`
- `query-state { reducer, key_b64?, consistency? }` → `{ state_b64?, meta{...} }` (reuse name but upgrade payload/metadata)
- `list-cells { reducer }` → `{ cells[], meta{...} }`
- `journal-head {}` → `{ journal_height, snapshot_hash?, manifest_hash }`
- `blob-get { hash_hex }` → `{ data_b64 }` (cap-checked; pairs with existing `put-blob`)

All control verbs should hit the introspection adapter (or CAS for blob-get) so daemon and batch paths share semantics and receipts can be replayed deterministically.

---

## Completed so far
- Added `sys/query@1` `defcap` to built-ins (`spec/defs/builtin-caps.air.json`) and exposed it via `CapType::QUERY` and builtins loader.
- Extended built-in schema/effect lists to include introspection params/receipts and `introspect.*` effects with `cap_type=query`.
- Updated test fixtures to grant `sys/query@1` by default and declare it in manifests alongside http/timer/blob caps.
- Implemented kernel-side internal handler for `introspect.*` with deterministic receipts; host run loop now intercepts and applies these without going through external adapters (preserving replay).
- Control path wired: `WorldHost::run_cycle` intercepts `introspect.*`, `ShadowExecutor` uses the same handler, and control server exposes `manifest-read`, `query-state`, `list-cells`, `journal-head`, and `blob-get` verbs returning consistency metadata.
- Added control client and CLI helpers for `manifest-read`, `query-state`, `list-cells`, and `blob-get`, decoding payloads and `ReadMeta` for daemon-first flows with batch fallback.
- Added kernel unit tests for introspection handler + integration tests (non-daemon) covering manifest/state/list-cells/journal-head paths.
- Policy gating verified: added plan-level tests that deny `introspect.*` via policy and reject missing `query_cap` grants.

---

## Effect Handler Design (kernel vs host)

### Options considered
- **Kernel-resident internal handler**: treat `introspect.*` as an in-kernel effect that never leaves the deterministic core. Kernel already owns the state, snapshots, and manifest hashes needed to answer queries without I/O. We can deterministically synthesize receipts straight from the journal/snapshot index, skipping any host plumbing. This keeps read-only paths close to the invariants and avoids lifetime/borrow gymnastics in `aos-host` (the host currently drains effects, drops the mutable borrow on the kernel, then runs async adapters).
- **Host adapter**: implement `AsyncEffectAdapter` in `aos-host` that calls back into `Kernel::get_manifest/get_reducer_state/list_cells`. This mirrors how HTTP/LLM adapters live today, but requires sharing a `StateReader` handle into the kernel across an async boundary (likely `Arc<Mutex<Kernel>>`), and we would still want deterministic receipts (so no external inputs). The layering becomes awkward because the host would need a privileged handle that can read snapshot metadata safely while the kernel is idle.

### Decision
Keep introspection (and future kernel-owned readonly utilities) **inside the kernel** as a small “internal adapter” surface. The host remains the orchestrator for external, effectful adapters; the kernel owns deterministic, zero-I/O handlers that can be replayed from receipts. This also generalizes to `p1-self-upgrade` where plans must read the manifest + consistency metadata while preparing patches.

### Implementation sketch
1) **Internal handler trait**: add a lightweight, synchronous trait (e.g., `InternalEffectHandler`) in `aos-kernel` that takes an `EffectIntent` and returns an `EffectReceipt`. It should never block or touch wall-clock; it’s purely a mapping from normalized params → receipt.
2) **Kernel dispatch**: extend `Kernel::drain_effects()` (or the host run loop) to intercept intents whose kind is in an `INTERNAL_EFFECTS` set (initially `introspect.manifest|reducer_state|journal_head|list_cells`). For these kinds, call the handler immediately and push the resulting receipt into the list that will be applied this cycle. Preserve intent order when interleaving internal and external receipts so replay alignment stays intact.
3) **Receipt shape**: use `adapter_id = "kernel.introspect"` (or per-effect ids if we want finer audit). `status = Ok` on success; on errors (e.g., `SnapshotUnavailable`, `ReducerMissing`, `InvalidConsistency`), emit `status = Error` with a structured payload `sys/IntrospectError@1 { code, message }`. Even error receipts carry `meta { journal_height, snapshot_hash?, manifest_hash }` so callers know what was consulted.
4) **Param decoding**: decode params into typed structs inside the kernel module (mirroring the defeffect schemas). Map textual `consistency` to `Consistency` enum, validate reducer exists, and when `Exact(h)` is requested try snapshot fallback; return `Error` if missing.
5) **Host integration**: no new `AsyncEffectAdapter` needed. The host run-cycle stays the same except it partitions intents: internal → handled synchronously via kernel, external → `AdapterRegistry::execute_batch`. This avoids adding any `Arc<Mutex<Kernel>>` dance to adapters. Control socket verbs for manifest/state/blob-get still delegate to the kernel entry points so CLI and plans observe identical receipts.
6) **Signing**: reuse the existing receipt signing stub, but record a distinct `adapter_id` for internal handlers so audits can distinguish them from host adapters.

### Why this fits the layering
- Effects are the abstraction boundary; receipts are the audit trail. An internal handler still emits receipts, so replay stays deterministic.
- Kernel already exposes `StateReader` + snapshot index; giving host adapters mutable/async access complicates borrow lifetimes and risks racing with later ticks. Keeping it in-kernel eliminates that class of bugs.
- Host remains for “side-effectful” adapters (http/llm/blob/timer), while kernel handles “world read” utilities. Future self-upgrade planning uses the same pattern.

### Follow-on tasks
- Add `internal_effects` registry + handler plumbing to `aos-kernel`.
- Implement `IntrospectionHandler` covering the four effects with shared helper to build `ReadMeta`.
- Update `WorldHost::run_cycle` to interleave internal receipts with external adapter receipts in intent order.
- Add control-socket verbs to call the kernel handler directly (no host adapter); CLI will rely on those.
- Tests: unit tests for handler error paths; integration tests that emit `introspect.*` intents via plans and assert receipts land in the journal with expected meta.
