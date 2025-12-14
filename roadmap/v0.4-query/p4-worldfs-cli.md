# WorldFS View Helpers

**Status**: Shelved — CLI implementation removed; kept here for reference if we revive the concept later. p4-introspection is DONE and can back future read commands.

This document defines CLI commands and LLM helper APIs that provide a filesystem-like view over AgentOS introspection surfaces. These are convenience wrappers—they do not add new capabilities.

---

## CLI Placement & Flow

- Command form: `aos world fs <op> <path>` (world-scoped like `world gov` / `world state`).
- Execution: prefer the control socket (daemon path), fall back to batch host + `StateReader` when the daemon is absent.
- Data sources: `introspect.*` effects + ObjectCatalog + `blob.get`; system/object responses include `journal_height`, `snapshot_hash`, `manifest_hash` for governance-grade provenance. `/blob/**` reads can only attach contextual read meta (from `introspect.journal_head`) because CAS blobs are not journaled.

---

## Path Model

WorldFS exposes a virtual namespace with three root prefixes:

```
/sys/**      System introspection (manifest, reducers, journal)
/obj/**      ObjectCatalog artifacts
/blob/**     Raw CAS blob access
```

### `/sys` — System Introspection

| Path | Description | Backed by |
|------|-------------|-----------|
| `/sys/manifest` | Current world manifest | `introspect.manifest` |
| `/sys/reducers/<Name>` | Reducer state root | `introspect.reducer_state` |
| `/sys/reducers/<Name>/<key>` | Specific reducer key | `introspect.reducer_state` |
| `/sys/journal/head` | Journal height + hashes | `introspect.journal_head` |

**Keyed reducers (cells)**: `ls /sys/reducers/<Name>` enumerates cell keys via `introspect.list_cells`; `cat /sys/reducers/<Name>/<key>` reads a single cell via `introspect.reducer_state` with `key`.

**Key encoding**: If the reducer key bytes are valid UTF-8, show them verbatim; otherwise render as `0x<hex>` and always expose the base64 form in `--long/--json` output. Inputs accept either UTF-8 text (decoded to bytes) or `0x<hex>`; raw base64 can be passed via `--key-b64` in CLI/API helpers.

### `/obj` — ObjectCatalog Artifacts

| Path | Description | Backed by |
|------|-------------|-----------|
| `/obj/` | List all objects | `ObjectCatalog` reducer |
| `/obj/<path>` | Object metadata (latest version) | `ObjectCatalog` reducer |
| `/obj/<path>/v<N>` | Specific object version metadata | `ObjectCatalog` reducer |
| `/obj/<path>/data` | Latest payload | `blob.get` via stored hash |
| `/obj/<path>/v<N>/data` | Payload for version N | `blob.get` via stored hash |

Objects use hierarchical path-like names (e.g., `agents/self/patches/0003`). Zero-padding numeric segments is **optional**; it just keeps lexicographic order aligned with numeric order in `ls`/`tree` outputs. Directory nesting is purely lexical: `/obj/foo/bar` is one object name shown under `foo/` in `tree`, but the payload is a single blob at `/obj/foo/bar/data`—no partial or nested payloads inside that object. If you also store `/obj/foo/data`, then `foo` will appear both as a file (with a payload) and as a directory that contains `bar`; this is allowed but discouraged because it complicates listings—prefer distinct prefixes when possible. For versioned series, prefer the explicit `/v<N>` suffix so version selection is unambiguous (`/obj/foo/v0002`), while the bare `/obj/foo` resolves to the latest version.

### `/blob` — Raw CAS Access

| Path | Description | Backed by |
|------|-------------|-----------|
| `/blob/<hash>` | Raw blob by content hash | `blob.get` |

---

## CLI Commands

### `aos world fs ls <path>`

List contents at path.

```bash
# List reducers
aos world fs ls /sys/reducers

# List keyed reducer cells
aos world fs ls /sys/reducers/Orders

# List objects by prefix
aos world fs ls /obj/agents/self/

# List all objects of a kind
aos world fs ls /obj --kind=air.patch

# Long/JSON output (shows hashes, sizes, versions)
aos world fs ls /obj/agents --long
aos world fs ls /obj/agents --json
```

**Flags**: `--kind <kind>`, `--tags tag1,tag2`, `--recursive` (default false), `--long` (show meta columns), `--json` (machine-readable), `--depth <n>` (tree depth for recursive lists), `--key-b64` (when targeting keyed reducers).

**Implementation**: Control verb → `introspect.*` or catalog helper; batch fallback queries `StateReader`/ObjectCatalog directly. For keyed reducers, `ls /sys/reducers/<Name>` calls `introspect.list_cells` to return cell keys (and optional size/hash metadata) instead of pulling full state. Object listings fetch `ObjectVersions.latest` + `versions[latest]` for each name; `--recursive` builds a virtual directory tree from shared prefixes.

### `aos world fs cat <path>`

Read content at path.

```bash
# Read manifest
aos world fs cat /sys/manifest

# Read reducer state
aos world fs cat /sys/reducers/Counter/count

# Read object payload
aos world fs cat /obj/agents/self/patches/0003/data

# Read raw blob
aos world fs cat /blob/sha256:abc123...

# Read specific object version payload
aos world fs cat /obj/agents/self/patches/0003/v2/data
```

**Implementation**: `introspect.manifest/reducer_state` + `blob.get` (via control); batch host fallback if daemon absent.

### `aos world fs stat <path>`

Show metadata without content.

```bash
aos world fs stat /obj/agents/self/patches/0003
# path: /obj/agents/self/patches/0003
# kind: air.patch
# hash: sha256:abc123...
# size: 4096
# created: 2024-01-15T10:30:00Z
# tags: [draft, v2]
# version: 3 (latest)
# journal_height: 42
# manifest_hash: ...
# snapshot_hash: ...
```

**Implementation**: Queries `ObjectCatalog` metadata only. `stat` on `/obj/...` resolves to latest version unless `/v<N>` is specified; returns both the resolved version and hashes. `stat` on `/sys/**` uses `introspect.*` receipts. `stat` on `/blob/<hash>` performs a `blob.get` (size derived from bytes) and may attach contextual read metadata (see metadata rules below); this does **not** attest when the blob was created.

### `aos world fs tree [path]`

Show hierarchical view.

```bash
aos world fs tree /obj
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

**Implementation**: Queries `ObjectCatalog` with a prefix filter, then materializes a virtual tree by splitting object names on `/`, grouping immediate children of the requested prefix, and sorting lexicographically. Only leaf objects have payloads (`.../data`); intermediate “directories” are synthesized from shared prefixes. Includes consistency metadata for the catalog read. `--depth` limits nesting; `--long/--json` include per-leaf meta (kind/hash/size/version).

### `aos world fs grep <pattern> <path>` (optional)

Search within objects.

```bash
aos world fs grep "error" /obj/agents/self/logs/
```

**Implementation**: Lists matching objects, loads payloads via `blob.get`, searches UTF-8 text; binary payloads are skipped unless `--binary` is set (then search raw bytes). Output lines include object path, version, and optional offset.

---

## LLM Helper APIs

Plans can use these helper operations for cleaner introspection code:

### `fs_ls(prefix: string, options?: LsOptions) -> Entry[]`

```typescript
interface LsOptions {
  kind?: string;      // Filter by object kind
  tags?: string[];    // Filter by tags
  recursive?: bool;   // Include nested paths
  depth?: number;     // Max depth when recursive
  include_versions?: bool; // list /v<N> entries per object
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
  if is_keyed_reducer(prefix):
    emit_effect(introspect.list_cells, {reducer: extract_name(prefix)})
  else:
    emit_effect(introspect.reducer_state, {name: extract_name(prefix)})
elif prefix starts with "/obj":
  query ObjectCatalog reducer with prefix filter (latest version unless /v<N>)
```
`is_keyed_reducer` is determined from the manifest (`cell_mode`/routing metadata). Object results include `version` and `hash` from `ObjectVersions`.

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

* System/Object responses include `journal_height`, `manifest_hash`, `snapshot_hash`.
* CAS blobs are not journaled, so `blob.get` has no provenance meta; helpers may issue `introspect.journal_head` first and attach that **read context** alongside the bytes. This indicates when the read happened, not when the blob was created.
* Agents should capture meta when preparing governance proposals.
* Stale reads are detectable via hash comparison.

---

## Implementation Notes

These helpers are implemented purely in terms of:

1. `emit_effect(introspect.*)` — for system introspection
2. `ObjectCatalog` reducer queries — for object metadata
3. `emit_effect(blob.get, ...)` — for payload retrieval

No new kernel functionality is required **now that p4-introspection has landed** (introspect effects + caps + control verbs). The helpers are sugar over existing primitives; the only delta is optional contextual meta for `blob-get` (CLI can synthesize via `journal_head` until/if the control verb returns it directly).
