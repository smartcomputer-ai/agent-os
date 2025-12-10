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
