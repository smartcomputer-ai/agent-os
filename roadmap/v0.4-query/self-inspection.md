# Self-Inspection via WorldFS

**Status**: Design spec for v0.4-query / v0.5-upgrade

---

## WorldFS Overview

**WorldFS is a *virtual*, semantic file system used by agents inside a world.**

It is built entirely from existing AgentOS mechanisms:

* **`introspect.*` effects** (read-only, consistency-aware)
* **`ObjectCatalog` reducer + `ObjectMeta` schema** (agent-generated artifacts)
* **CAS blob store** (payloads)
* **Agent descriptors** stored as objects or in a registry reducer

WorldFS is *not* a kernel-level filesystem and has no POSIX semantics.
It is a structured, inspectable namespace for agents to discover the world's structure and their own artifacts, making self-upgrade possible.

---

## Self-Inspection Surfaces

A self-upgrading agent uses four read-only surfaces:

### 1. Manifest View `/sys/manifest`

Backed by `introspect.manifest` (StateReader � CAS node + metadata).

Returns the current world manifest including modules, plans, schemas, caps, and policies.

### 2. Reducer State `/sys/reducers/<Name>/<key>`

Backed by `introspect.reducer_state`.

Allows querying specific reducer state by name and key path.

### 3. Catalog Objects `/obj/...`

Backed by the `ObjectCatalog` reducer and CAS blobs.

Agent-generated artifacts are stored here with hierarchical path-like names.

### 4. Journal Metadata `/sys/journal/head`

Optionally backed by `introspect.journal_head`.

Provides journal height and related metadata for consistency reasoning.

---

**Consistency guarantee**: Every introspection response includes **journal_height**, **manifest_hash**, and **snapshot_hash** so the agent can reason deterministically about what it saw and attach these to a governance proposal.

---

## ObjectCatalog: Versioned Artifacts Inside a World

All agent-generated artifacts (code bundles, AIR patches, prompts, test results, metrics, diagnostics, worldgraphs, etc.) are stored as **objects**:

* **Write path:**
  `bytes � blob.put � hash � ObjectRegistered event � ObjectCatalog updated`

* **Naming:** hierarchical path-like names (e.g., `agents/self/patches/0003`), but not enforced by kernel.

* **`kind` field** lightly types the object (`"air.patch"`, `"code.bundle"`, `"log"`, &).

* **`tags`** provides extensible, LLM-friendly metadata.

ObjectCatalog is the **writeable part of the virtual filesystem**, and the only one accessible to agents.

**Note:**
* Object payloads live in CAS
* ObjectCatalog stores only metadata + version history

---

## Self-Upgrade Workflow (FS-based view)

1. **Inspect**
   * Read `/sys/manifest`
   * Read own descriptor under `/agents/<name>/descriptor`
   * Query relevant reducer states under `/sys/reducers/&`
   * Discover past patches / code bundles in `/obj/...`

2. **Design**
   * LLM: propose AIR patch or code changes
   * Write design specs to `/obj/design/<id>`

3. **Produce Artifacts**
   * AIR patches � `/obj/air.patch/<id>`
   * Code bundles � `/obj/code.bundle/<id>`

4. **Build & Test** (optional)
   * `build.module` produces WASM hash + logs � `/obj/build.log/<id>`

5. **Governance Commit**
   * Create governance proposal referencing artifacts in `/obj`
   * Submit via `governance.propose/shadow/approve/apply`

---

## Capabilities Required by a Self-Upgrade Agent

| Cap | Purpose |
|-----|---------|
| **Query Cap** | Call `introspect.*` effects |
| **Blob Cap** | Write object payloads to CAS |
| **Catalog Cap** | Submit `ObjectRegistered` events |
| **Build Cap** | Run `build.module` effect |
| **Governance Cap** | Access `governance.*` effects |
| **LLM Cap** | Code synthesis and reasoning |

> A self-upgrade agent cannot modify the world except through governance.
> All other surfaces are strictly read-only or append-only.

---

## Safety Principles

* WorldFS is **append-only** except via governance.
* Introspection effects are read-only and must be capability-gated.
* ObjectCatalog entries **cannot** be deleted or overwritten, only versioned.
* Agents must attach introspection metadata to governance proposals.
* Agents may not propose patches changing their own capabilities (unless allowed by a higher-risk governance pathway).

---

## Self-Model

The upgrade agent must be able to discover:

* its plans
* its reducers
* the code bundles implementing those
* its capabilities & policies
* its risk classification and governance limits

All retrieved via:

* `/agents/<self>/descriptor`
* `/obj` lookups referencing code bundles
* `/sys/manifest` referencing module hashes

---

## Relationship to P1 (Self-Upgrade)

This spec provides the **read surface** that P1's governance effects build upon. The self-upgrade loop requires:

1. **This spec (v0.4)**: introspection effects + ObjectCatalog for inspection and artifact storage
2. **P1 (v0.5)**: governance effects (`propose/shadow/approve/apply`) for committing changes

Together they enable governed self-modification where agents can propose, rehearse, and apply their own manifest patches under explicit caps/policy.
