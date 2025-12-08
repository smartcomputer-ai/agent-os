# P1 – Query Catalog (World Object / Artifact Catalog)

**Status:** Draft
**Scope:** Single world
**Audience:** Runtime implementers, agent authors, CLI / tooling authors

---

## 1. Motivation

Self-upgrading worlds (and any non-trivial agents) need a way to:

* **Store artifacts** they produce (code bundles, AIR patches, logs, diagnostics, datasets, prompts…)
* **Discover and query** those artifacts later (by name, kind, tags, time…)
* **Reference artifacts** in governance proposals and plans

We want this to feel like a “file system” for the world, but we don’t want:

* A new kernel-level FS subsystem
* POSIX semantics (rename/move/delete)
* Tight coupling to any specific storage backend

Instead, we define a **Query Catalog**:

> A **versioned catalog of named objects** inside a world, backed by a normal reducer (`ObjectCatalog`), with payloads stored in CAS / blob store.
> Agents read and write via events and existing effects, and higher-level views (WorldFS, CLI “fs”) sit on top.

This doc specifies that catalog.

---

## 2. Goals & Non-Goals

### 2.1 Goals

* Provide a **uniform way to register and discover artifacts** inside a world.
* Be **LLM-friendly**: names, kinds, tags that make sense to an agent.
* Keep the model **append-only and auditable**.
* Reuse existing primitives:

  * CAS / blob store for payload bytes
  * Reducers for state
  * Effects for I/O and introspection
* Integrate cleanly with:

  * `p1-self-upgrade` (as the agent’s workspace)
  * `p1-query-interfaces` (as a queryable surface)

### 2.2 Non-Goals (P1)

* No general search/index system (full-text, vector search) – can be added later.
* No deletes or renames (beyond “soft delete” via metadata).
* No cross-world / universe catalog – this is per-world only.
* No filesystem kernel changes – all semantics live in AIR + reducers.

---

## 3. Conceptual Model

Conceptually, the Query Catalog is:

* A **key/value space of objects** inside a world.
* Each object has:

  * A **logical name** (path-like string)
  * A **kind** (lightweight type tag, e.g. `code.bundle`, `air.patch`, `log`)
  * A **payload hash** (CAS / blob ref)
  * A **set of tags** (free-form labels)
  * Some metadata (created_at, owner, etc.)
  * A **version** (monotonically increasing per name)

Formally:

* There is a **reducer** `sys/ObjectCatalog@1` keyed by `name : text`.
* The value for a given `name` tracks:

  * `latest` version number
  * a map of **version → ObjectMeta**

Agents:

* **Write** by:

  * `blob.put` → `hash`
  * raising an `ObjectRegistered@1` event
* **Read** by:

  * introspecting `ObjectCatalog` state (`introspect.reducer_state`)
  * fetching object payloads via `blob.get(hash)`

CLI/SDK “fs” helpers and the WorldFS view map:

* `/obj/<kind>/<name>` → Query Catalog entries

---

## 4. Data Schemas

### 4.1 `sys/ObjectMeta@1`

Metadata for a single object version.

```jsonc
{
  "$kind": "defschema",
  "name": "sys/ObjectMeta@1",
  "type": {
    "record": {
      "name":       { "text": {} },    // logical name, e.g. "agents/self/patches/0003"
      "kind":       { "text": {} },    // coarse type tag, e.g. "code.bundle", "air.patch"
      "hash":       { "hash": {} },    // payload in CAS / blob store
      "tags":       { "set": { "text": {} } },
      "created_at": { "time": {} },
      "owner":      { "text": {} }
    }
  }
}
```

Notes:

* `name` is world-local. We encourage path-like names (`a/b/c`) but do not enforce hierarchy.
* `kind` is descriptive, not a schema ref. Later, we can add `schema: Name` if we want typed facts.
* `tags` is intentionally generic: domain-specific layers can define conventions later.

### 4.2 `sys/ObjectVersions@1` (per name state)

Each catalog key is a set of versions.

```jsonc
{
  "$kind": "defschema",
  "name": "sys/ObjectVersions@1",
  "type": {
    "record": {
      "latest":   { "nat": {} },  // latest version number for this name
      "versions": {
        "map": {
          "key":   { "nat": {} },           // version number
          "value": { "ref": "sys/ObjectMeta@1" }
        }
      }
    }
  }
}
```

Semantics:

* `latest` starts at 0 or 1 (implementation choice) and monotonically increases.
* Adding a new object version increments `latest` and inserts into `versions`.

### 4.3 Reducer: `sys/ObjectCatalog@1`

Reducer state:

* Key: `name : text`
* Value: `sys/ObjectVersions@1`

The reducer reacts to events (defined below) to update its rows.

---

## 5. Events & Write Path

### 5.1 `sys/ObjectRegistered@1` event

Agents **register** an object by emitting a domain event:

```jsonc
{
  "$kind": "defschema",
  "name": "sys/ObjectRegistered@1",
  "type": {
    "record": {
      "meta": { "ref": "sys/ObjectMeta@1" }
    }
  }
}
```

The `ObjectCatalog` reducer listens for this event and:

1. Looks up the existing `ObjectVersions` for `meta.name` (or initializes if missing).
2. Increments `latest` to `v = latest + 1`.
3. Inserts `versions[v] = meta` (with `meta.name` matching the key).
4. Emits no further effects; pure state update.

#### 5.1.1 Invariants

* `meta.hash` MUST refer to a blob that exists in the CAS/blob store.

  * The runtime MAY enforce this by validating at event time or lazily.
* `meta.name` MUST match the reducer key (`name`) for this state.
* Versions are append-only; no in-place mutation of `meta` is allowed.

### 5.2 Recommended write pattern for agents

Agents do not directly modify `ObjectCatalog` state; they follow this pattern:

1. **Write payload** (if there is one):

   ```text
   payload_bytes → effect blob.put → hash
   ```

2. **Prepare metadata**:

   ```text
   meta = {
     name:       "agents/self/patches/0003",
     kind:       "air.patch",
     hash:       hash,
     tags:       {"self-upgrade", "patch", "v3"},
     created_at: now,
     owner:      "sys/self-upgrade@1"   // agent name or principal id
   }
   ```

3. **Register object**:

   * Raise an `ObjectRegistered@1` event into `sys/ObjectCatalog@1` with `{ meta }`.

4. **Record references** as needed:

   * In other reducers (e.g. `AgentRegistry`) or in governance proposals.

---

## 6. Read Path & Query Patterns

The catalog is read via:

* **Reducer introspection** (`introspect.reducer_state`) for structured queries.
* **Blob reads** (`blob.get`) for payload bytes.

### 6.1 FS-like access (WorldFS / CLI / SDK)

The virtual FS / CLI “fs” helpers present the catalog as:

```text
/obj/<kind>/<name>/
  meta      # ObjectMeta (latest)
  payload   # contents of blob(hash)
```

But this is a **view layer only**. The underlying access is:

1. Call `introspect.reducer_state` on `sys/ObjectCatalog@1` to get:

   * For a given `name` (keyed lookup) → one `ObjectVersions`.
   * Or for the whole reducer (full map of name → ObjectVersions).

2. Filter at the client:

   * By `kind`
   * By tags
   * By created_at, owner, etc.

3. When you choose a `meta.hash`:

   * Call `blob.get(hash)` to get payload bytes.

### 6.2 Plan-side helper patterns

We expect a small library for plans with helpers like:

* `catalog_list(kind?, tags?, prefix?) → [ObjectMeta]`
* `catalog_latest(name) → (version, ObjectMeta)`
* `catalog_versions(name) → [(version, ObjectMeta)]`
* `catalog_read_payload(meta) → bytes`

All implemented with:

* `emit_effect(introspect.reducer_state)`
* `emit_effect(blob.get)`

No kernel privileges required.

---

## 7. Capabilities & Policy

The Query Catalog touches two capability domains:

1. **Blob access** – to store and retrieve payload bytes.
2. **Catalog access** – to emit `ObjectRegistered` events and to introspect `ObjectCatalog` state.

### 7.1 Blob capabilities

The existing blob effect family (`blob.put`, `blob.get`, etc.) is governed by its own cap type (e.g. `sys/blob.cap@1`). The catalog spec assumes:

* Only plans with `blob.put` cap can create payloads.
* Only plans with `blob.get` cap may read raw bytes.

### 7.2 Catalog capabilities

Two aspects:

1. **Write:** who can register objects?

   * The event `ObjectRegistered@1` should have a dedicated **domain capability** or be guarded by the reducer’s module cap.
   * The policy can restrict which agents can emit that event.

2. **Read:** who can introspect catalog state?

   * Introspection uses `introspect.reducer_state` with `CapType "query"`.
   * Policies can allow/restrict introspection of `sys/ObjectCatalog@1` per plan/agent.

Default recommendation:

* `ObjectCatalog` lives in a `sys/` module with strict caps:

  * Only designated “writers” (self-upgrade agent, certain system agents) can register objects.
  * Any agent with query cap may **read** catalog state as long as policy allows.

---

## 8. Integration with Self-Upgrade & Query Interfaces

### 8.1 Self-Upgrade (p1-self-upgrade)

The self-upgrade agent uses the catalog as its “workspace”:

* **AIR patches**: `kind = "air.patch"`
* **Code bundles**: `kind = "code.bundle"`
* **Build logs / diagnostics**: `kind = "build.log"`, `"build.diagnostics"`
* **Design briefs / reports**: `kind = "design.doc"`
* **Worldgraph snapshots**: `kind = "world.graph"`

The upgrade loop becomes:

1. Inspect manifest & state via `introspect.*`.
2. Read relevant objects from catalog `/obj/...`.
3. Generate new artifacts; write via `ObjectRegistered`.
4. Reference catalog objects in governance proposals.

### 8.2 Query Interfaces (p1-query-interfaces)

The catalog is just another reducer visible via the standard Query Interface:

* `StateReader.get_reducer_state("sys/ObjectCatalog@1", key=…)`
* HTTP query surfaces may expose:

  * `GET /state/sys.ObjectCatalog/<name>` → JSON form of `ObjectVersions`
  * `GET /state/sys.ObjectCatalog` → full catalog (careful with size)

This keeps the Query Interface **pure**: the catalog is simply one of the queryable states.

---

## 9. Invariants & Safety

The following invariants should be explicitly maintained:

* **Append-only metadata:**

  * Versions never change; new versions are added; old ones are immutable.
* **Content addressing:**

  * `meta.hash` always refers to a payload in CAS/blob.
* **Auditability:**

  * All writes are journalled as `ObjectRegistered` events.
  * Introspection of `ObjectCatalog` returns metadata including:

    * `journal_height`
    * `manifest_hash`
    * `snapshot_hash`
* **Capability discipline:**

  * Only authorized agents can register objects or read payloads.

---

## 10. Future Extensions (Non-blocking for P1)

The P1 spec deliberately avoids overreach but leaves room for:

* **Indices / search reducers:**

  * Secondary indices by kind/tag/owner/time.
* **TTL / retention policies:**

  * Soft retention on certain kinds (logs, diagnostics).
* **Universe-level / cross-world catalogs:**

  * A separate “universe catalog” reducer indexing per-world catalogs.
* **Typed facts:**

  * Objects that additionally declare a `schema: Name` and are treated as first-class facts.

None of these are required for the self-upgrade MVP; they are straightforward on top of this spec.

---

## 11. Summary

The Query Catalog is:

* A **simple, typed, versioned object directory** inside a world.
* Implemented as a normal reducer + events + blob store.
* Accessed via existing introspection and blob effects.
* Foundation for:

  * Self-upgrade agents’ workspace,
  * WorldFS `/obj/…` view,
  * Future universe-level sharing.

It gives you the “file system feel” you want, while staying inside the AOS event-sourced, content-addressed, capability-governed model.

