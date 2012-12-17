# P4: CLI Refresh (resource-first, no WorldFS)

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

### manifest / defs
- `manifest get [--raw] [--consistency ...]` (optional meta; pairs with `state` semantics).
- `manifest stat` fast path: manifest hash + head height + snapshot hash.
- `defs get <name>` reads a `defschema/defcap/defeffect` entry from the active manifest (JSON only; consider `--raw` for canonical form if useful).

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
- Control `step` removed; keep CLI surface aligned and ensure no tooling depends on it.
- Confirm `ObjectCatalog` reducer/API matches the desired `obj` UX (versions, tags, depth view).
- Ensure `defs get` is implemented (control verb + batch path) so schema/cap/effect defs are addressable like other resources.
