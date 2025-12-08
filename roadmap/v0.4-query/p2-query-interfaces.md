# Read-only query surfaces

This note sketches an opt-in read path for worlds. Reducers still own all mutations via event→replay, but callers can observe materialized state without emitting new events. The design keeps determinism and auditability while allowing slightly stale reads when callers accept them.

## Principles
- **Deterministic origins**: Every response is derived from a replayed snapshot + optional journal tail at a declared height.
- **Zero mutation**: Query handlers never append to the journal or emit effects/receipts.
- **Capability-gated**: Read access is governed by the same capability/policy model as effects, even though it is read-only.
- **Optional surfaces**: The in-process API is always available; the HTTP adapter is opt-in and wired via manifest/policy.

## Data sources and freshness
- **Hot path**: In-memory cache of the latest replayed snapshot (state hash + manifest hash + journal height). Cheap reads that may be a few events behind if replay is running.
- **Warm path**: On-disk snapshot plus a small journal tail replayer when callers request `at_least_height` freshness and the tail is available.
- **Cold path**: Historical snapshots in CAS for point-in-time queries and debugging.
- **Contract**: Every response includes `(journal_height, snapshot_hash, manifest_hash)` so callers know what they read. Clients can request `exact_height` (fail if behind) or `at_least_height` (serve newest available ≥ requested or return the cached head).

## Entry point: StateReader trait
Implement a `StateReader` trait inside the kernel process that exposes read-only accessors:
- `get_reducer_state(module_id, key, consistency)` for keyed reducer state.
- `get_manifest(consistency)` for control-plane inspection (modules, plans, capabilities, policies).
- `get_journal_head()` for metadata only (height + hashes).

`consistency` captures caller preference: `Exact(height)`, `AtLeast(height)`, or `Head`. Implementations resolve via the hot path first, then warm path replay, and include the resolved height/hash in the response envelope.

## How external callers learn the height
- **Response metadata**: Every read returns `(journal_height, snapshot_hash, manifest_hash)`; the caller need not know the height beforehand.
- **Optimistic freshness**: Clients that merely want “latest available” send `Head` and observe the returned height for bookkeeping.
- **Consistency-sensitive flows**: Clients that must correlate with a known event or receipt can supply `Exact(height)` or `AtLeast(height)`; the service either replays the tail to reach that height or fails fast, letting callers retry later.
- **Repeatable reads**: When the same height is requested again, the service can serve the cached snapshot or cold path without re-executing reducers, ensuring consistent views during audits.

## Fit with replay/audit philosophy
- Reads reuse canonical CBOR decoding and snapshot formats, so replay guarantees hold and reducers remain the single source of truth for state evolution.
- No new event types or receipts are introduced; observability comes from metrics/logs that include the returned height/hash.
- Capability gating on the query surface prevents unbounded data exfiltration and aligns with the existing manifest-driven governance model.


## Semantic vs Observational Reads (Design Note)

AgentOS distinguishes between:

1. **Observational Reads**
- Performed via `StateReader` or its HTTP adapter
- Never append to the journal 
- Suitable for external clients, dashboards, and inter-world inspection when the consumer does _not_ use the data in world logic 
- Return consistency metadata (`journal_height`, snapshot/manifest hashes)
        
2. **Semantic Reads**
- Reads whose results influence reducer/plan behavior
- Must be performed via an effect (e.g., `world.query`)
- Produce receipts recorded in the journal to maintain deterministic replay
- Required for cross-world reads that world A depends on
        
This separation ensures efficient observability while preserving deterministic semantics.
