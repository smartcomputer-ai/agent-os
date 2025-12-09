# WorldFS View Helpers

**Status**: Ergonomics layer for v0.4-query (CLI work beyond minimal `aos world state` flags is **deferred here**).

This document defines CLI commands and LLM helper APIs that provide a filesystem-like view over AgentOS introspection surfaces. These are convenience wrappers—they do not add new capabilities.

---

## Path Model

WorldFS exposes a virtual namespace with four root prefixes:

```
/sys/**      System introspection (manifest, reducers, journal)
/obj/**      ObjectCatalog artifacts
/blob/**     Raw CAS blob access
/agents/**   Agent descriptors and metadata
```

### `/sys` — System Introspection

| Path | Description | Backed by |
|------|-------------|-----------|
| `/sys/manifest` | Current world manifest | `introspect.manifest` |
| `/sys/reducers/<Name>` | Reducer state root | `introspect.reducer_state` |
| `/sys/reducers/<Name>/<key>` | Specific reducer key | `introspect.reducer_state` |
| `/sys/journal/head` | Journal height + hashes | `introspect.journal_head` |

### `/obj` — ObjectCatalog Artifacts

| Path | Description | Backed by |
|------|-------------|-----------|
| `/obj/` | List all objects | `ObjectCatalog` reducer |
| `/obj/<path>` | Object metadata | `ObjectCatalog` reducer |
| `/obj/<path>/data` | Object payload | `blob.get` via stored hash |

Objects use hierarchical path-like names (e.g., `agents/self/patches/0003`).

### `/blob` — Raw CAS Access

| Path | Description | Backed by |
|------|-------------|-----------|
| `/blob/<hash>` | Raw blob by content hash | `blob.get` |

### `/agents` — Agent Descriptors

| Path | Description | Backed by |
|------|-------------|-----------|
| `/agents/` | List registered agents | Registry reducer or manifest |
| `/agents/<name>/descriptor` | Agent descriptor | Registry reducer or `/obj` |

---

## CLI Commands

### `aos fs ls <path>`

List contents at path.

```bash
# List reducers
aos fs ls /sys/reducers

# List objects by prefix
aos fs ls /obj/agents/self/

# List all objects of a kind
aos fs ls /obj --kind=air.patch
```

**Implementation**: Calls `introspect.*` or queries `ObjectCatalog` reducer.

### `aos fs cat <path>`

Read content at path.

```bash
# Read manifest
aos fs cat /sys/manifest

# Read reducer state
aos fs cat /sys/reducers/Counter/count

# Read object payload
aos fs cat /obj/agents/self/patches/0003/data

# Read raw blob
aos fs cat /blob/sha256:abc123...
```

**Implementation**: Calls appropriate introspection effect, then `blob.get` for payloads.

### `aos fs stat <path>`

Show metadata without content.

```bash
aos fs stat /obj/agents/self/patches/0003
# kind: air.patch
# hash: sha256:abc123...
# size: 4096
# created: 2024-01-15T10:30:00Z
# tags: [draft, v2]
# version: 3
```

**Implementation**: Queries `ObjectCatalog` metadata only.

### `aos fs tree [path]`

Show hierarchical view.

```bash
aos fs tree /obj
# /obj
# ├── agents/
# │   └── self/
# │       ├── patches/
# │       │   ├── 0001
# │       │   └── 0002
# │       └── logs/
# │           └── build-001
# └── shared/
#     └── schemas/
```

**Implementation**: Queries `ObjectCatalog`, formats as tree.

### `aos fs grep <pattern> <path>` (optional)

Search within objects.

```bash
aos fs grep "error" /obj/agents/self/logs/
```

**Implementation**: Lists matching objects, loads payloads via `blob.get`, searches content.

---

## LLM Helper APIs

Plans can use these helper operations for cleaner introspection code:

### `fs_ls(prefix: string, options?: LsOptions) -> Entry[]`

```typescript
interface LsOptions {
  kind?: string;      // Filter by object kind
  tags?: string[];    // Filter by tags
  recursive?: bool;   // Include nested paths
}

interface Entry {
  path: string;
  kind: "dir" | "file" | "object";
  meta?: ObjectMeta;
}
```

**Implementation**:
```
if prefix starts with "/sys/reducers":
  emit_effect(introspect.reducer_state, {name: extract_name(prefix)})
elif prefix starts with "/obj":
  query ObjectCatalog reducer with prefix filter
elif prefix starts with "/agents":
  query registry reducer or ObjectCatalog
```

### `fs_read(path: string) -> bytes`

Read content at path, resolving through CAS if needed.

**Implementation**:
```
meta = fs_stat(path)
if meta.hash:
  emit_effect(blob.get, {hash: meta.hash})
else:
  emit_effect(introspect.*, appropriate_params)
```

### `fs_stat(path: string) -> Stat`

```typescript
interface Stat {
  path: string;
  exists: bool;
  kind?: string;
  hash?: string;
  size?: number;
  version?: number;
  tags?: string[];
  journal_height: number;    // Consistency metadata
  manifest_hash: string;
  snapshot_hash: string;
}
```

### `fs_list_objects(kind?: string, tags?: string[]) -> ObjectMeta[]`

Convenience wrapper for filtered object listing.

**Implementation**: Queries `ObjectCatalog` reducer with filters.

### `fs_read_reducer(mod: string, key?: string) -> Value`

Read reducer state directly.

**Implementation**: `emit_effect(introspect.reducer_state, {name: mod, key: key})`

### `fs_read_manifest() -> Manifest`

Read current world manifest.

**Implementation**: `emit_effect(introspect.manifest, {})`

---

## Safety Guidance

### Capability Requirements

FS helpers do **not** bypass capability requirements:

| Helper | Required Cap |
|--------|--------------|
| `fs_ls`, `fs_read`, `fs_stat` on `/sys/**` | Query Cap |
| `fs_ls`, `fs_read`, `fs_stat` on `/obj/**` | Query Cap + Blob Cap (for payloads) |
| `fs_read` on `/blob/**` | Blob Cap |

### Read-Only by Design

* All `fs_*` helpers are **read-only**
* Writing to `/obj/**` requires going through `ObjectCatalog` event pathway
* No FS helper can modify world state

### Consistency Guarantees

* All responses include `journal_height`, `manifest_hash`, `snapshot_hash`
* Agents should capture these when preparing governance proposals
* Stale reads are detectable via hash comparison

---

## Implementation Notes

These helpers are implemented purely in terms of:

1. `emit_effect(introspect.*)` — for system introspection
2. `ObjectCatalog` reducer queries — for object metadata
3. `emit_effect(blob.get, ...)` — for payload retrieval

No new kernel functionality is required. The helpers are sugar over existing primitives.
