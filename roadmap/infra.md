# Infra

## 1) The core invariants to preserve in hosting

From the spec, the runtime wants:

* **Journal is authoritative** (append-only, canonical CBOR).  
* **Baselines are semantic restore roots** (restore = baseline snapshot + journal tail). 
* **CAS is immutable `{hash → bytes}`** and **GC roots come only from snapshot-pinned roots** (no mutable “named refs”). 
* **Worlds should be movable** between workloads (a worker process can run multiple worlds; orchestrator assigns). 

Hosting must therefore provide:

* A **shared CAS** (dedupe, integrity, immutable)
* A **shared durable journal** (ordered, append-only)
* A **shared snapshot store** (and snapshot indexing)
* A **durable queue system** for receipts, timers, inboxes, etc. (so worlds can move or crash without losing in-flight work)

---

## 2) Recommended persistence stack (pragmatic + scalable)

You asked “persistence tech? thinking FoundationDB…” 
I’d do:

### A. FoundationDB (FDB) for *metadata + ordering + queues*

FDB is excellent for:

* atomic multi-key updates
* ordered keyspaces (journal/inbox queues)
* leases
* durable queues (single system for both persistence + queue, which you asked for) 

### B. Object store (S3/GCS/R2) for *large immutable blobs*

Use object store for:

* CAS blob bodies (especially large)
* snapshot bodies
* WASM modules, big workspaces, large artifacts

FDB stores:

* small objects inline (optional)
* pointers to object-store locations for large blobs
* indexes and roots

This matches the “CAS + snapshot_ref pointers” approach described (journal holds pointers for snapshots, etc.). 

### C. Local disk cache on workers

Every worker keeps an LRU cache of CAS objects by hash, because worlds replay and reducers load cell state blobs frequently (cells store per-cell state in CAS). 

---

## 3) Data model: Universe → Worlds → (CAS, Journal, Snapshot, Queues)

You said: “Worlds live in universes… shared CAS/journals/snapshots.” 
So define a **Universe** as a namespace + isolation boundary:

* CAS is **shared within a universe** (dedupe across worlds, cheap copy/move).
* Journal + snapshot infra is “shared service” but logically per-world.

### 3.1 Universe IDs and World IDs

* `universe_id`: UUID
* `world_id`: UUID (or UUIDv7 if you want roughly time-ordered creation)

World address = `(universe_id, world_id)`.

### 3.2 World Creation Baseline Requirement

Every world must have an `active_baseline` at creation time.

* unseeded create: write an initial baseline snapshot for empty/default runtime state
* seeded/forked create: set `active_baseline` to the selected seed/fork snapshot

---

## 4) CAS in hosting: how to persist blobs, nodes, snapshots, cell state

The specs make CAS central: AIR nodes, blobs, receipts, snapshots, cell state blobs, workspace trees, etc. are content-addressed by sha256 of canonical bytes.  

### 4.1 CAS API (logical)

You want a small interface all services use:

* `cas_put(bytes) -> hash`
* `cas_get(hash) -> bytes`
* `cas_has(hash) -> bool`

“Put” must **verify** hash matches bytes (never trust client).

### 4.2 CAS storage layout

**FDB subspace:**

* `cas/<universe>/<hash> -> { size, location, inline_bytes? }`

**Object store:**

* `cas/<universe>/sha256/<hex>` as the canonical blob body

Policy:

* if `len(bytes) <= N` (e.g., 16KB), store inline in FDB
* else store in object store; FDB keeps pointer + size

### 4.3 CAS and Cells (keyed reducers)

Cells v1.1 explicitly says:

* cell state stored as CAS blobs
* per keyed reducer keep a content-addressed `CellIndex` whose **root hash** is stored in world state/snapshots 

Hosting implication:

* you must assume **a lot of CAS traffic** (cell state blobs + index nodes)
* local cache on workers matters
* snapshots must include `cell_index_root` per reducer (see snapshots below)

### 4.4 CAS GC strategy (important)

Cells spec is explicit: **no named refs; GC walks from snapshot-pinned roots**. 
So implement GC as a **mark-and-sweep**:

Root set per world:

* “active manifest hash” (pins AIR nodes + referenced WASM hashes, schemas, etc.)  
* latest K snapshots (pins snapshot blob + all reducer/cell/workspace roots inside snapshot)
* optionally: “retention window” for recent journal segments (if you store segments in CAS)

Then:

* mark all reachable CAS hashes by traversing snapshot content (and other pinned roots)
* sweep unmarked CAS objects older than grace period

This is easiest if snapshot format is explicit about “root hashes referenced” (see snapshot format below).

---

## 5) Journal persistence: how to store “append-only CBOR log” in hosted form

The architecture doc describes:

* journal is append-only
* segment files, monotonic sequence numbers
* events are length-prefixed canonical CBOR
* snapshots referenced by journal entries 

In hosting, you can either:

1. literally store “segment blobs” (log-structured files) in object store, or
2. store events as ordered KV entries (simpler for a DB like FDB)

I’d do **(2) KV journal + optional segment compaction**, because you also need inbox queues and atomic scheduling.

### 5.1 World journal keyspace

In FDB:

* `world/<universe>/<world_id>/journal/<height> -> entry_cbor_bytes`
* `world/<universe>/<world_id>/journal_head -> height`

Where `height` is a monotonic `u64` (nat) used in call context (`sys/ReducerContext@1` includes `journal_height`). 

### 5.2 Who is allowed to append to the journal?

To keep ordering and avoid contention, I strongly recommend:

**Only the “world host” (the worker holding the lease) appends to the journal.**

Everything else (cross-world messages, adapter receipts, control-plane injections, timers) goes into a **durable inbox queue**, and the world host drains it and appends in order.

This avoids multi-writer conflicts and makes `journal_head` increments cheap and conflict-free.

It also matches the spirit of the current control channel verbs (`event-send`, `receipt-inject`)—those are “enqueue a DomainEvent / inject a receipt,” not “write arbitrary log bytes yourself.” 

### 5.3 Append operation (atomic batch)

A world “tick” often appends multiple derived entries (DomainEvents, EffectIntents, Policy decisions, etc.). 

So define one primitive:

`append_batch(world_id, entries[]) -> first_height`

Implementation:

* read `journal_head = h`
* write entries at `h+1 .. h+n`
* update `journal_head = h+n`

All in one transaction (FDB transaction).

This guarantees: **a tick’s entries are contiguous and ordered**.

### 5.4 Streaming / tail

For debug/ops, you’ll eventually want “tail journal.” The control spec explicitly says journal streaming is deferred today, but it’s a known direction. 

In hosted mode:

* implement tail by scanning `(world_id, height)` keys
* optionally add “server-side follow” via FDB watches or a pubsub layer

---

## 6) Snapshot persistence: what to store, how to index, how to restore

Architecture doc: snapshots persist control-plane AIR state + reducer state bytes + pinned blob roots. Restore is always baseline snapshot + replay tail.

Cells doc adds: snapshots persist `cell_index_root` per keyed reducer. 

### 6.1 Snapshot blob format (recommended)

Store snapshot as a CBOR blob in CAS:

```cbor
SnapshotV1 = {
  version: 1,
  universe_id,
  world_id,
  journal_height,
  manifest_hash,
  // reducer instances:
  reducers: [
    {
      reducer_name,
      // unkeyed reducer state:
      state_cbor?: bytes,
      // keyed reducer state represented via CAS-backed index root:
      cell_index_root?: hash,
    },
    ...
  ],
  // plan runtime (instances, waits, locals, etc.)
  plans: { ... },
  // effect manager runtime (pending intents, idempotency cache, etc.)
  effects: { ... },
  // any other pinned roots (workspace roots typically live inside Workspace reducer state)
  pinned_roots: [hash...]
}
```

Key point: include enough data to restart without scanning the whole journal:

* plan runtime state (awaits, step outputs)
* pending effects / idempotency fences (so effects don’t get “lost” on restart)
* cell_index_roots

### 6.2 Snapshot indexing

In FDB:

* `world/<u>/<w>/snapshots/<height> -> snapshot_hash`
* `world/<u>/<w>/active_baseline -> {height, snapshot_hash, receipt_horizon_height?}`

And you also append journal entries:

* `Snapshot { snapshot_ref, height }` when snapshotting  
* `BaselineSnapshot { snapshot_ref, height, logical_time_ns, receipt_horizon_height? }` when promoting a baseline

### 6.3 Restore algorithm

When a worker acquires a world lease:

1. read `active_baseline`
2. `cas_get(snapshot_hash)`
3. hydrate world runtime state
4. replay journal entries with `height >= baseline.height` in order
5. resume normal stepping

This matches the intended semantics. 

---

## 7) Durable queues in hosting: inboxes, outboxes, timers, receipts

This is the part that makes “worlds movable” and crash-tolerant.

### 7.1 World inbox queue (the key primitive)

Everything arriving *into* a world should be an ordered, durable queue:

* cross-world messages
* adapter receipts
* external inboxes (`routing.inboxes`)  
* control channel injections (`event-send`, `receipt-inject`) 
* timer firings

FDB structure:

* `world/<u>/<w>/inbox/<seq> -> InboxItem`
* `world/<u>/<w>/inbox_head -> seq` (optional)
* `world/<u>/<w>/notify -> counter` (for watch/wakeup)

Use `seq` as a monotonic sortable token (could be FDB versionstamp or ULID). Multiple writers can enqueue without contending on a single counter if you use versionstamps.

**InboxItem** could be a tagged union like:

* `DomainEventIngress { schema, value_cbor, key? }`
* `ReceiptIngress { intent_hash, adapter_id, payload_cbor }`
* `InboxIngress { source, payload_cbor }` (for `routing.inboxes`) 
* `TimerFiredIngress { ... }`

The world host drains inbox in `(seq)` order, converts to canonical journal entries (validates/canonicalizes like normal), appends, then deletes inbox items.

This keeps the journal single-writer and deterministic.

### 7.2 Effect outbox queue (dispatch)

Even though EffectIntents are journaled, adapters need a dispatch queue.

Option A (simple): world host directly calls adapters (HTTP, LLM, etc.) and then enqueues receipts back into inbox.

* works, but ties effect execution to world placement

Option B (recommended for “factory scale”): **separate adapter workers**

* world host writes EffectIntents to a global durable queue
* adapter workers consume intents and enqueue receipts to world inbox

Queue keys:

* `universe/<u>/effects/pending/<seq> -> { world_id, intent_hash, kind, params_cbor, ... }`

This aligns with the architecture: effect manager queues intents, adapters execute, receipts rejoin via journal.  

### 7.3 Timer service (durable)

Timer.set is a micro-effect and must fire even if a world migrates or restarts. 

So implement timer scheduling as a durable queue keyed by `deliver_at`:

* `universe/<u>/timers/<deliver_at_ns>/<intent_hash> -> { world_id, ... }`

A timer worker:

* scans near-future buckets
* when due, enqueues `TimerFiredIngress` into destination world inbox
* (optional) adds signature/adapter_id so it looks like a normal receipt event (matching `sys/TimerFired@1` patterns)  

This is consistent with “timers are effects with receipts that are journaled.” 

---

## 8) World scheduling and moving worlds between workers

You said: “worker process could run multiple worlds … moving them between workloads.” 
So you need:

### 8.1 Leases

A world is “owned” by one worker at a time:

* `world/<u>/<w>/lease -> { worker_id, expires_at }`

Worker renews periodically. If it can’t renew, it stops hosting the world.

### 8.2 Orchestrator

Orchestrator responsibilities:

* maintain inventory of workers (capacity, locality, loaded worlds)
* decide placement
* assign/revoke leases

For simplicity: orchestrator writes desired assignment:

* `world/<u>/<w>/desired_worker -> worker_id?`

Workers watch for assignments matching themselves and attempt to acquire lease.

### 8.3 Wakeups (“ready queue”)

When inbox gets new items, the world should get scheduled quickly. Options:

* **FDB watch** on `world/<u>/<w>/notify` (increment on enqueue)
* Or: also publish best-effort wakeup to NATS/Redis pubsub (not authoritative; just latency optimization)

Given you want “queue system ideally same as persistence” , you can start with FDB watches and add pubsub later if needed.

---

## 9) Cross‑world messaging: durable, idempotent, auditable

You asked specifically: “how to do cross world messaging?”

### 9.1 Treat cross-world messaging as an effect + receipt

Why: it’s external I/O from a world’s perspective. The architecture is very consistent: external actions happen via effect intents and signed receipts.  

So introduce a **Fabric adapter** inside a universe, with effect kind like:

* `fabric.send` (plan-only, typically)
* params: `{ dest_world, schema, value_cbor (or blob_ref), key? }`
* receipt: `{ status, message_id, dest_world, enqueued_seq }`

Then the adapter:

1. validates authorization (cap/policy; you can model a new cap type like `fabric.msg`)
2. enqueues an inbox item into destination world inbox
3. returns a receipt to the sender (receipt goes to sender’s inbox; sender host appends to journal)

### 9.2 The two delivery modes

You’ll probably want both:

#### Mode A: “Typed event delivery”

* message contains `{schema: Name, value_cbor}`
* destination world host appends it as a normal `DomainEvent` journal entry
* routing/triggers work normally  

This is ideal if cooperating worlds share schemas.

#### Mode B: “Inbox delivery” (generic envelope)

Use `manifest.routing.inboxes` to route cross-world messages to a dedicated reducer (e.g., `sys/FabricInbox@1`) without requiring shared domain schemas.

Message envelope schema could be stable and built-in like:
`sys/FabricMessage@1 = { from_world, from_universe, payload_schema?, payload_cbor, headers }`

This is ideal for multi-tenant / plugin-style worlds.

### 9.3 Idempotency + dedupe (critical)

You must ensure “send” is safe under retries and failures.

Best practice:

* set `message_id = intent_hash` (or include intent_hash in the message)
* in Fabric adapter, write a **dedupe key**:

`world/<u>/<dest>/fabric_dedupe/<message_id> -> seq`

Transaction logic:

* if dedupe key exists: return receipt indicating “already enqueued” with the existing seq
* else:

  * write inbox item at a fresh seq
  * set dedupe key
  * bump notify key

This prevents duplicate inbox delivery even if adapter retries.

### 9.4 Ordering semantics

Within a destination world:

* inbox seq order defines delivery order
* world host appends in that order
* journal order becomes the authoritative “happened-before” for that world

Between worlds:

* there is no global order (and you don’t want one)
* if you need causal ordering, include causal metadata (source world + source journal height) in message payload and let reducers enforce.

### 9.5 “Request/response” across worlds

Don’t add RPC. Do it as:

* A sends message with `correlation_id`
* B processes and sends response message back carrying that `correlation_id`
* A’s plan can `await_event` on a response event filtered by correlation id (the workflow patterns already emphasize correlation keys to avoid cross-talk).  

---

## 10) How this maps to the existing control-plane verbs

Locally, you already have:

* `event-send` and `receipt-inject` that enqueue and run a cycle 

In hosted mode:

* “event-send” becomes: write a `DomainEventIngress` into world inbox (not directly into journal), wake worker.
* “receipt-inject” becomes: write `ReceiptIngress` into world inbox.

Same semantics, just networked and durable.

---

## 11) Minimal DoD path (aligned with your DoD list)

Your DoD for hosting: shared CAS, shared journal infra, shared snapshot infra, orchestrator. 

Here’s the “ship it fast but correct” sequence:

1. **Universe CAS service**

   * object store backend + sha256 verification
   * local worker cache
   * no GC initially (time-based retention)

2. **World journal in FDB**

   * KV log by height + head pointer
   * append_batch API (single-writer via lease)

3. **World inbox queue**

   * multi-writer enqueue (messages/receipts/control)
   * single-writer drain into journal

4. **Snapshot store**

   * snapshot blob in CAS
   * snapshot index + journal Snapshot entry
   * restore logic in worker

5. **Orchestrator + worker leases**

   * worker can host N worlds
   * lease-based ownership + recovery

6. **Fabric adapter**

   * implements cross-world messaging by inbox enqueue + dedupe
   * optionally add caps/policy later

Once this is in place, you can run “hundreds of universes / thousands of worlds” without changing world semantics. 

---

If you want, next I can write out:

* a concrete **FDB subspace layout** (tuple keys) for all of the above
* a precise **transaction protocol** for `drain_inbox -> append_journal_batch -> enqueue_effects -> maybe_snapshot`
* or a spec-level proposal for the **fabric.send** effect kind + schemas (so it plugs neatly into AIR’s typed effect model). 


---

# Log compaction

Yes — but **not inside FDB as “rewrite the ordered keys”** (that’s basically a full rewrite + hot range churn). I’d do **log compaction as an “exported segment layer”** to object storage, and keep FDB as the authoritative *index + recent tail*. This preserves the “journal is authoritative append-only” model  while preventing FDB from becoming your long-term blob store. 

### The practical pattern: “hot tail in FDB, cold segments in object store”

**In FDB (hot):**

* `journal/<height> -> entry_bytes` for the last N entries or last T days
* `journal_head -> height`
* `active_baseline -> (height, snapshot_hash, receipt_horizon_height?)` 

**In object store (cold):**

* `segments/<world_id>/<start_height>-<end_height>.log` (canonical CBOR, length-prefixed exactly like local segments) 
* optionally also `segments_index/<end_height> -> { start, end, object_key, checksum }` in FDB

**Compactor job (per world, or sharded):**

1. Pick a compaction window `[h0..h1]` where `h1 < active_baseline_height` and receipt-horizon safety checks pass.
2. Stream those FDB entries in order, write a segment file to object store.
3. Atomically write a `segments_index` record (FDB) that says “this range is materialized in object store”.
4. Then delete the individual `journal/<height>` keys for that range from FDB.

This gives you **bounded FDB growth** while keeping deterministic replay: restore uses `active_baseline` then replays **object-store segments + FDB tail** to head.

### Why not “in-place compaction” in FDB?

FDB doesn’t love “rewrite a huge ordered key range into a new structure and delete the old” if you do it aggressively; you’ll pay in:

* write amplification
* range hotspots
* transaction size limits (you’ll chunk anyway)
* operational complexity

The segment export approach converts “compaction” into **append-only object writes + delete old tail keys**, which is much friendlier.

### Retention policy: what can you safely compact away?

Given AgentOS semantics:

* **Active baseline + retained tail are sufficient to restart** + replay forward. 
  So you can compact everything **below** the active baseline height.
* If you keep, say, **K baselines per world**, you can keep journal segments back to the oldest retained baseline height, and delete older segments entirely (unless you want infinite audit).

If you want *infinite audit*, keep all segments forever in object store (cheap) and only bound FDB.

### A tiny but important detail: “journal is authoritative”

Even if you export segments, the journal remains the source of truth — you’re just changing its **physical layout** from “many keys” to “few segment blobs”. That matches the architecture’s “segment files with monotonic sequence numbers.” 

### Suggested defaults

* **FDB tail**: last 50k–500k entries (tunable) *or* last 1–7 days
* **Snapshot frequency**: time-based (e.g., every few minutes) + size-based
* **Compaction trigger**: once a new snapshot is committed, compact everything older than `(snapshot_height - margin)`.

If you want, I can sketch the exact keyspaces + the restore algorithm that seamlessly merges: `(snapshot) + (segments) + (fdb tail)` and show how this interacts with your inbox/outbox design so “world migrates” stays clean. 


---

# Forking & Branching

Branching can be *really* clean in your model if you lean into the fact that **CAS is immutable** and **snapshots are just roots**. In FDB-only (plus object store for big blobs), branching mostly becomes **metadata + “copy-on-write from a snapshot root”**, not “clone a whole database.”

## The mental model

A “world” is basically:

* a **journal** (append-only)
* occasional **snapshots** (each snapshot is a CAS root that captures all reducer/cell/workspace roots at some `journal_height`)  
* a **current manifest ref** and other small metadata

Because CAS is shared per-universe, branching can reuse *all* existing bytes.

So branching = **create a new world whose starting point is an existing snapshot (or journal height)**, then append new entries.

---

## Recommended branching primitive: “fork from snapshot”

### Operation: `world_fork(src_world, src_height_or_snapshot) -> new_world`

Implementation (all in one FDB transaction):

1. Create `new_world_id`
2. Set `new_world.active_baseline = (src_height, snapshot_hash, receipt_horizon_height?)`
3. Set `new_world.journal_head = src_height`
4. Create a **new empty journal tail** (no entries above head)
5. Copy/point the manifest (or let it diverge later)
6. Initialize world inbox/outbox metadata empty

No CAS copy. No reducer copy. You’re just pointing the new world at the same snapshot root.

**Restore behavior for the new world:** load baseline snapshot, replay entries with `height >= baseline.height` — which is empty beyond the fork point — so it boots exactly into the branched state.

**Complication:** you must ensure the snapshot is “complete enough” to restart without scanning earlier history (it should be, by design).

---

## What about forking from a journal height without a snapshot?

You can support it, but it’s more annoying:

* If you fork at `height H` and there’s no snapshot at/near H, restore requires replay from the closest earlier snapshot (or from genesis), which can be expensive.

So the standard pattern is:

* **ensure there’s a snapshot at the fork point** (or take one as part of fork).

I’d implement fork as:

* if user requests `height H`, the host either:

  * finds latest snapshot `<= H`, or
  * **forces a snapshot at H** (by replaying to H in a worker, snapshotting, then committing the fork metadata)

But to keep fork cheap and synchronous, prefer: **fork only from snapshots**.

---

## Seeding: “create world from template”

Seeding is just “fork from a template world’s snapshot.”

Have a “template world” per universe (or per project):

* stable snapshot roots (e.g., “base runtime + standard reducers + workspace skeleton”)
* maybe multiple named seeds (dev/prod/test)

Then `world_create(seed=template@snapshot)` is identical to fork.

This makes “create and seed worlds easily” almost free.

---

## How much does this complicate the backend?

Not much, but you need to decide a few semantics up front:

### 1) Snapshot format must be branch-safe

Two key requirements:

* Snapshot contains everything needed to restart deterministically: reducer states, keyed reducer `cell_index_root`, plan runtime state, pending effect/idempotency info, workspace roots, etc.  
* Snapshot must not embed “world-unique identity” in a way that breaks determinism after fork. (If it does, define a reducer/system field that updates on first tick, not in snapshot.)

### 2) World identity and causality

After fork:

* `world_id` changes
* you may want metadata: `parent_world_id`, `parent_snapshot_hash`, `forked_at_height`
  This is purely for introspection; doesn’t affect runtime.

### 3) Effects and external side effects

Forking raises a real question: **what about pending effects / receipts?**
You generally want:

* Forked world should not “inherit” in-flight external side effects in a way that causes duplication.

Best practice:

* In snapshot, store **pending effect intents** as part of state (so crash recovery works),
* But on fork, you can apply a **fork policy**:

  * either “reset effect manager state” (drop pending intents, clear idempotency cache)
  * or “keep but fence with a new world epoch” so adapters treat them as distinct and won’t re-run old intents.

I’d choose:

* **On fork: clear pending intents and idempotency cache by default** (safe for branching in dev/testing).
* If you need “exact replay including effects,” that’s a different mode (“replay world”), not a normal branch.

This is the biggest semantic decision branching introduces.

### 4) CAS GC must understand branches

If CAS GC roots are “latest K snapshots per world,” branching adds more worlds → more roots → more retention. That’s fine, but:

* you likely need **quotas**: max worlds per tenant, max snapshots pinned, etc.
* keep GC based on “pinned snapshot roots across all worlds.”

No extra complexity beyond “more roots.”

### 5) Compaction / segment export

Fork-from-snapshot doesn’t complicate compaction:

* each world has its own journal tail and segments
* they share the snapshot blob and CAS objects

---

## Nice-to-have: cheap “branch of branch” and named refs

You’ll want friendly names like:

* `world/main`
* `world/feature-x`
* `world/seed/base`

Implement as separate metadata:

* `refs/<universe>/<name> -> world_id` (mutable)
  and keep the world itself immutable in identity.

This is not required for correctness, but it’s how you make branching feel like Git.

---

## Summary recommendation

* Make **fork-from-snapshot** the core primitive.
* Treat **seeds as template snapshots**.
* Decide a clear default for **effects on fork** (I recommend “clear pending effects + idempotency state”).
* Backend complexity impact is modest: mostly metadata + a couple policy choices.

If you want, I can write the exact FDB key layout + a single-transaction “fork” pseudocode, plus the effect-fencing options (world epoch / adapter dedupe keys) so you can choose the semantics you want.
