# P4: CLI Refresh (read UX without WorldFS)

**Goal**: Clean, capability-honest CLI for reads/writes; no faux filesystem. Standardize flags/output, make keyed reducers easy, and unify daemon/batch behavior.

## Command Surface (proposed)
- `aos world state get <reducer> [--key <utf8>|--key-hex <HEX>|--key-b64 <B64>] [--consistency head|exact:H|at-least:H] [--json|--raw]`
- `aos world cells ls <reducer> [--json]`
- `aos world obj ls [prefix] [--kind K --tag T --depth N --versions --json]`
- `aos world obj get <name> [--version N] [--raw|--json]`
- `aos world obj stat <name> [--version N] [--json]`
- `aos world blob get <hash> [--raw] [--json|--meta]`
- Keep existing `event/run/gov/init/head/manifest/snapshot/put-blob`; add `doctor` for quick world diagnostics.

## UX Principles
- One verb set: `ls/get/stat`; no POSIX path fiction.
- Provenance everywhere: `{journal_height, snapshot_hash, manifest_hash}` emitted uniformly (stderr for human mode; embedded for JSON).
- Stable output: tables by default; `--json` for machine; never mix data/meta on stdout.
- Key handling: accept utf8/hex/b64 flags; always CBOR-encode keyed reducer keys internally (ObjectCatalog, etc.).
- Daemon-first, batch fallback with a single “using batch” notice; same consistency semantics via a `ReadClient` trait.

## Implementation Steps
1) Infra: add `ReadClient` abstraction used by all read verbs (control client + batch host impls).
2) Renderers: shared human table + JSON serializers with optional meta block; strict separation stdout/stderr.
3) Add key helpers: `KeyInput::Utf8/Hex/B64 -> Vec<u8>`; CBOR encode for keyed reducers.
4) Implement `obj` verbs over ObjectCatalog + blob.get; support versions and depth view (lexical tree, not POSIX).
5) Implement `blob get` (hash parsing with/without `sha256:`); reuse meta from journal head.
6) Refactor `state` to use new client/renderer; add key-hex/b64 flags.
7) Wire `cells ls` to reuse renderers and key display; ensure raw JSON option.
8) Add `doctor` for env/store/manifest/control-socket sanity.
9) Update docs/examples to new verbs; remove WorldFS references; add shell completions tests.
10) Tests: golden CLI snapshots (daemon + batch) for state/obj/blob/cells; keyed reducer coverage; consistency meta asserted.

## Non-goals
- No POSIX-like `fs/ls/cat/tree` veneer.
- No deletes/renames; ObjectCatalog remains append-only.
- No HTTP read surface in this milestone (future).

## Migration Notes
- Mark old `fs` command retired; point users to `obj/blob/state`.
- Update example 09 text to show `obj ls/get` once implemented.
