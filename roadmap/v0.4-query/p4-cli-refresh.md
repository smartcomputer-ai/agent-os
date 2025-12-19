# P4: CLI Refresh

**Complete**

**Goal**: Ship a resource-oriented CLI with uniform verbs/flags, daemon-first reads, and sane keyed-reducer ergonomics—without the old `world` prefix or a faux filesystem.

## North Star
- Resource-oriented nouns (`state/manifest/obj/blob/gov/journal/snapshot/defs`), not a POSIX-ish tree. Single verb set `ls/get/stat` for reads.
- Provenance is only attached where it is meaningful: reducer state reads (and optional manifest reads). Human mode prints meta to stderr; JSON mode nests it. Other commands stay clean.
- Daemon-first, batch fallback with one “using batch” notice; same consistency semantics either way.
- Keys are easy: use `key_schema` + routing `key_field` to derive/encode keys for `state get` and `event send`, with manual overrides.

## CLI Shape
### Grammar
`aos [GLOBAL_OPTS] <noun> <verb> [args...]`

World resolution (for world-scoped nouns):
1. `--world PATH`
2. `AOS_WORLD`
3. Walk up from `cwd` to find a world root marker (e.g., `.aos/`)

### Universal flags (everything else becomes envs)
- `-w, --world <PATH>`
- `--mode <auto|daemon|batch>` (default auto)
- `--control <PATH>` (default `<world>/.aos/control.sock`)
- `--json` (machine output; stable schema)
- `--pretty` (pretty JSON; implies `--json`)
- `--quiet` (suppresses notices like “using batch”)
- `--timeout <DURATION>` (client-side control timeout)
- `--no-meta` (state/manifest JSON only)

Advanced knobs prefer env vars (`AOS_MODE`, `AOS_CONTROL`, `AOS_TIMEOUT`, etc.) instead of one-off flags.

### Output contract
- Human: stdout = primary data; stderr = provenance + notices. No tables mixed with meta.
- JSON: single object `{ data, meta?, warnings? }` on success, `{ error, meta? }` on failure. Meta only appears for state/manifest reads.

## Command Surface (final shape)
```
aos
  init
  status
  run
  stop

  event    send
  receipt  inject              (advanced)

  state    get
           ls

  obj      ls
           get
           stat

  blob     put
           get
           stat

  manifest get
           stat

  defs     get                 (fetch defschema/defcap/defeffect entries)

  journal  head
           replay              (experimental; tail reserved)

  snapshot create              (alias: snapshot)
           ls                  (reserved)

  gov      propose
           shadow
           approve
           apply
           ls
           show
           diff                (reserved)

  doctor

  version
  completion
```

Notes:
- `step` is dropped (and should be removed from control unless needed for tests).
- Aliases remain temporarily for compatibility: `info→status`, `head→journal head`, `replay→journal replay`, `state→state get`, `cells→state ls`, `put-blob→blob put`, `shutdown→stop`, `manifest→manifest get`, `event→event send`, `snapshot→snapshot create`.

## Command Notes
### init / status / run / stop
- `run` defaults to daemon; `--batch` runs once and exits; `--detach` optional.
- `status` reports resolved world paths, daemon reachability, journal head, active manifest hash, last snapshot hash.
- `stop` uses control `shutdown {}`; ensure sockets aren’t clobbered if unhealthy.

### event send
- `aos event send <schema> <json|@file|@-> [--key ...] [--wait head|at-least:<H>]`
- If routing declares `key_field`, CLI derives key, encodes via reducer `key_schema`, and sends without user-provided bytes. Overrides: `--key <utf8>`, `--key-json <json>`, `--key-hex`, `--key-b64`.
- **Why encode keys client-side?** The CLI already has the manifest; pulling `key_schema` there lets us (1) route deterministically with canonical key bytes before the kernel decodes payloads, (2) keep one canonical envelope across ingress paths, and (3) fail fast when the routing field is missing or ill-typed. Complexity is contained because only `key_schema` (not full payload schema) is needed, it’s cached with the manifest, and explicit overrides remain as escape hatches.
- Error contract: keyed routes missing the declared `key_field` fail client-side; overrides still require a routing entry and will error if none exists.

### state
- `aos state get <reducer> [--key ...] [--consistency head|exact:<H>|at-least:<H>] [--json|--pretty|--raw]`
- `aos state ls <reducer> [--raw-keys] [--limit N]` (keyed reducers list keys; unkeyed returns empty with warning).
- Provenance meta attached here; `--no-meta` suppresses in JSON. Human mode prints meta to stderr.
- Decodes state using reducer schema when available; `--raw` streams canonical CBOR bytes.

### obj (backed by `ObjectCatalog`)
- `ls [prefix] [--kind K] [--tag T] [--depth N] [--versions]`
- `stat <name> [--version N|--latest]`
- `get <name> [--version N] [--raw|--out PATH]` (default: pretty JSON if CBOR+schema; refuse binary to TTY unless `--raw`/`--out`).
- Review `ObjectCatalog` API to confirm it matches desired CLI surface (versions, tags, depth view).

### blob
- `put <path|@->` → prints hash (JSON `{hash,size}`).
- `get <hash> [--raw|--out PATH] [--meta]` (default human: metadata only; require `--raw`/`--out` for bytes).
- `stat <hash>` → size/existence (debug-friendly).
- Safety default: without `--raw` or `--out`, `blob get` returns metadata plus a notice and never emits bytes.

### manifest / defs
- `manifest get [--raw] [--consistency ...]` (optional meta; pairs with `state` semantics).
- `manifest stat` fast path: manifest hash + head height + snapshot hash.
- `defs get <name>` reads a `defschema/defcap/defeffect/defmodule/defplan/defpolicy` entry from the active manifest (JSON only; consider `--raw` for canonical form if useful).
- `defs ls [--kind schema|module|plan|cap|effect|policy|secret]... [--prefix STR] [--json|--pretty]` lists defs from the active manifest, sorted by name. Human: table `KIND | NAME | DETAIL` (cap→cap_type; effect→params/receipt schema refs; module→reducer; plan→steps count; policy→rules count; schema empty). JSON: `{ data: { defs: [ { kind, name, cap_type?, params?, receipt? } ] }, meta }` with `--no-meta` to drop meta.

### journal / snapshot
- `journal head` returns height (and optional meta if paired with manifest reads).
- `journal replay [--to <H>] [--from <H>]` stays experimental; `journal tail` reserved for future streaming.
- `snapshot create` (alias `snapshot`) triggers `snapshot {}`; print resulting snapshot hash. `snapshot ls` reserved for a future listing.

### governance
- `gov propose [--patch FILE|--patch-dir DIR|--stdin] [--description ...] [--require-hashes] [--dry-run]`
- `gov shadow --id N`, `gov approve --id N [--decision approve|reject] [--approver NAME]`, `gov apply --id N` (prints new manifest hash).
- `gov ls/show` list and inspect proposals; `gov diff` reserved.

### doctor
- Checks world root layout, store/journal/snapshot dirs, manifest load/hash, control socket health/perms; prints remediation hints.

## Open Follow-ups
- Confirm `ObjectCatalog` reducer/API matches the desired `obj` UX (versions, tags, depth view). Flags are accepted; batch path supports prefix/depth/limit with notices when filters need daemon/catalog.
- Obj local fallback is batch-only via host state read; no daemon-less listing yet (batch path now returns keys with a notice).

## Progress to Date
- CLI flattened to `aos <noun> <verb>` with new nouns: `event`, `state`, `manifest`, `journal`, `snapshot`, `gov`, `defs`, `blob`, `obj`, `run/stop/status/init`.
- Universal flags wired: `--mode`, `--control`, `--json/--pretty/--quiet/--no-meta`, world walk-up resolution, control timeout. Output contract implemented via shared renderer (data on stdout, meta/notices on stderr; JSON `{data, meta?, warnings?}`).
- Control-first with batch fallback for state/manifest/journal/snapshot/obj/blob; daemon-required mode errors if socket missing; batch fallback warns unless `--quiet`.
- Verb map finalized and legacy control/CLI verbs removed (`step`, old aliases); help/completions aligned to the new surface.
- Key handling: added key overrides (`--key`, `--key-json`, `--key-hex`, `--key-b64`) with schema-based encoding for keyed reducers; `state get` uses encoded keys; `event send` derives keys from routing `key_field` + reducer `key_schema` with override escape hatches and clear error contracts.
- New nouns implemented: `defs get/ls` (control with local manifest fallback), `blob put/get/stat` (safe TTY defaults), `obj ls/get/stat` via ObjectCatalog (control-first; batch fallback for get/stat; batch ls supports prefix/depth/limit and warns when advanced filters are ignored).

## Implementation Plan (CLI Refresh)
1) Control + batch plumbing: finalize verb map (no `step`, no legacy aliases), add `defs get` control/batch paths, ensure `send-event`/`inject-receipt` run a cycle, and align daemon/batch meta payloads. **(done)**
2) CLI foundation: implement universal flags + world resolution, shared control/batch client wrapper, and unified output rendering (stdout data, stderr meta/notices; JSON envelope with optional meta). **(done)**
3) Event/state ergonomics: add key derivation/encoding (`key_schema` + routing `key_field`) for `event send`, key parsing for overrides, `state get/ls` consistency handling, and schema-aware state decoding with `--raw`. **(done)**
4) Object/blob/manifest/defs surfaces: build `obj ls/get/stat` atop `ObjectCatalog` + `blob get` safeguards, implement blob put/get/stat UX defaults, and wire `manifest get/stat` + `defs get` from control/batch with meta where applicable. **(done for blob/manifest/defs; obj flags accepted, batch ls supports prefix/depth/limit, richer catalog filters still to confirm)** 
5) UX polish + coverage: completions, CLI help text, update docs to the new nouns/verbs (no aliases), and golden/control tests for state/obj/blob/manifest/defs flows (daemon-first with batch fallback, provenance asserted). **(partially done: help smoke test + blob/obj basic tests; completions not versioned)**
