# P4: CLI Developer Experience ✅

**Status:** Complete

**Goal:** Make `aos world` the single, ergonomic CLI surface for controlling worlds. Replace the planned REPL with an improved CLI that maps cleanly to control channel verbs and supports governance.

## Rationale

The original REPL design (P4-repl-dx) proposed a line-oriented interactive shell. After review:

1. **Low value-add over CLI**: The REPL commands (`event`, `state`, `step`, etc.) map 1:1 to control verbs—no scripting language, no live programming, just command dispatch.
2. **Duplicated UX surface**: Maintaining both CLI subcommands and REPL commands doubles the documentation and testing burden.
3. **CLI-first is more composable**: Shell scripts, CI pipelines, and tooling integrate better with standalone commands than an interactive session.

A true REPL would make sense if it provided a live programming environment (e.g., an expression language or Ink-style terminal UI). Until that exists, invest in CLI ergonomics instead.

## Principles

- **Single namespace**: All world-specific operations live under `aos world`.
- **Global world resolution**: Avoid repeating `<path>` on every command. Use `--world`, `AOS_WORLD` env, or CWD detection.
- **Control channel parity**: Every control-surface CLI verb corresponds to a single control verb—same mental model everywhere.
- **Daemon-aware**: Commands check for a running daemon first; fall back to batch mode when appropriate.
- **Governance under `world`**: Governance acts on a specific world's manifest, so it belongs under `aos world gov`, not a separate top-level namespace.

## World Resolution Rules

For every `aos world …` command:

1. If `--world <DIR>` / `-w <DIR>` is passed → use that.
2. Else if `AOS_WORLD` env var is set → use that.
3. Else if CWD looks like a world (contains `air/`, `.aos/`, or `manifest.air.json`) → use `.`.
4. Else → error: "no world specified; pass `--world` or set `AOS_WORLD`".

## Global World Options

These flags apply to all `aos world` subcommands and can be set via env vars:

```text
aos world [WORLD_OPTS] <command> ...

WORLD_OPTS:
  -w, --world <DIR>            World directory (env: AOS_WORLD)
      --air <DIR>              AIR assets directory (env: AOS_AIR, default: <world>/air)
      --reducer <DIR>          Reducer crate directory (env: AOS_REDUCER, default: <world>/reducer)
      --store <DIR>            Store/journal directory (env: AOS_STORE, default: <world>/.aos)
      --module <NAME>          Module name to patch with compiled WASM
      --force-build            Force reducer recompilation
      --http-timeout-ms <N>    Override HTTP adapter timeout
      --http-max-body-bytes <N> Override HTTP adapter max response body
      --no-llm                 Disable LLM adapter
```

## Command Tree

```text
aos
└── world
    ├── init [PATH] [--template <name>]
    ├── info
    ├── run [--batch] [--reset-journal]
    ├── event <schema> (<json>|@file|@-) [--batch]
    ├── state <reducer> [--key <json>] [--raw]
    ├── snapshot
    ├── head
    ├── manifest [--raw]
    ├── put-blob @file [--namespace <ns>]
    ├── shutdown
    └── gov
        ├── propose --patch @file.patch.json
        ├── shadow --id <proposal-id>
        ├── approve --id <proposal-id> [--decision approve|reject]
        ├── apply --id <proposal-id>
        ├── list [--status pending|approved|applied|rejected|all]
        └── show --id <proposal-id>
```

## Command Specifications

### Lifecycle Commands

#### `world init [PATH] [--template <name>]`

Create a new world directory with skeleton manifest and store structure.

- `PATH` defaults to `.` if omitted.
- `--template` chooses a starter manifest (counter, http, llm-chat, etc.).

#### `world info`

Display read-only summary: manifest hash, journal head, active adapters, store location.

#### `world run [--batch] [--reset-journal]`

Start the world.

- **Without `--batch` (daemon mode)**:
  - Refuses to start if a daemon is already running (control socket exists and is healthy).
  - Runs long-lived with real timers and adapters.
  - Logs events/effects/receipts to console.
  - Ctrl-C triggers graceful shutdown with final snapshot.

- **With `--batch`**:
  - Errors if a daemon is already running.
  - Runs in batch mode: processes until quiescent, then exits.
  - Useful for CI, scripts, and replaying events.

Options:
- `--reset-journal` — clear journal before starting.

### Control-Surface Commands

These commands first attempt to use the control channel (if daemon is running), then fall back to batch mode where semantically valid.

#### `world event <schema> (<json>|@file|@-) [--batch]`

Enqueue a domain event.

- `@file` reads JSON from a file path.
- `@-` reads JSON from stdin.
- **Without `--batch`**: Enqueues the event only. If daemon running, sends via control channel. If no daemon, writes directly to journal.
- **With `--batch`**: Enqueues the event then runs until quiescent. Errors if daemon is running (batch mode only).

Control verb: `send-event` (enqueue only).

#### `world state <reducer> [--key <json>] [--raw]`

Query reducer state.

- `--key` for future keyed reducers (cells).
- Output is pretty-printed JSON by default; `--raw` outputs compact JSON.

Control verb: `query-state`.

#### `world snapshot`

Force a snapshot.

Control verb: `snapshot`.

#### `world head`

Return journal head (sequence number, hash) for health checks.

Control verb: `journal-head`.

#### `world manifest [--raw]`

Display the active manifest.

- `--raw` dumps canonical CBOR/JSON without formatting.

Control verb: `query-manifest` (if implemented) or read from store.

#### `world put-blob @file [--namespace <ns>]`

Upload a blob to the world's CAS, return the `HashRef`.

Control verb: `put-blob`.

#### `world shutdown`

Send graceful shutdown to a running daemon.

- Errors if no daemon is running.

Control verb: `shutdown`.

### Governance Commands

Governance commands live under `aos world gov` since they operate on a specific world's manifest/patch stream.

#### `world gov propose --patch @file.patch.json`

Submit a governance proposal.

- Validates patch against `patch.schema.json`.
- Returns `proposal_id` and `patch_hash`.

Control verb: `propose`.

#### `world gov shadow --id <proposal-id>`

Run shadow evaluation of a proposal.

- Returns shadow report hash and summary.

Control verb: `shadow`.

#### `world gov approve --id <proposal-id> [--decision approve|reject]`

Record approval or rejection decision.

- `--decision` defaults to `approve`.

Control verb: `approve`.

#### `world gov apply --id <proposal-id>`

Apply an approved proposal (commits new manifest root).

Control verb: `apply`.

#### `world gov list [--status pending|approved|applied|rejected|all]`

List governance proposals.

- `--status` filters by proposal status (default: `pending`).

Control verb: `gov-list` (read-only introspection).

#### `world gov show --id <proposal-id>`

Show details of a specific proposal.

Control verb: `gov-show` (read-only introspection).

## Control Verb Mapping

| CLI Command | Control Verb | Notes |
|-------------|--------------|-------|
| `event` | `send-event` | Enqueue only (daemon or direct journal) |
| `event --batch` | N/A | Batch mode: enqueue + run until quiescent |
| `state` | `query-state` | |
| `snapshot` | `snapshot` | |
| `head` | `journal-head` | |
| `manifest` | `query-manifest` | May read store directly if verb not implemented |
| `put-blob` | `put-blob` | |
| `shutdown` | `shutdown` | |
| `gov propose` | `propose` | |
| `gov shadow` | `shadow` | |
| `gov approve` | `approve` | |
| `gov apply` | `apply` | |
| `gov list` | `gov-list` | |
| `gov show` | `gov-show` | |

## Implementation Notes

### Clap Structure

```rust
#[derive(Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: TopCmd,
}

#[derive(Subcommand)]
pub enum TopCmd {
    World(WorldCommand),
    // Future: Module, Universe, Air
}

#[derive(Args)]
pub struct WorldCommand {
    #[command(flatten)]
    pub opts: WorldOpts,  // global world options

    #[command(subcommand)]
    pub cmd: WorldSubcommand,
}

#[derive(Args)]
pub struct WorldOpts {
    #[arg(short, long, global = true, env = "AOS_WORLD")]
    pub world: Option<PathBuf>,

    #[arg(long, global = true, env = "AOS_AIR")]
    pub air: Option<PathBuf>,

    // ... other global options
}

#[derive(Subcommand)]
pub enum WorldSubcommand {
    Init { path: Option<PathBuf>, template: Option<String> },
    Info,
    Run(RunArgs),
    Event(EventArgs),
    State(StateArgs),
    Snapshot,
    Head,
    Manifest { raw: bool },
    PutBlob(PutBlobArgs),
    Shutdown,
    Gov(GovCommand),
}

#[derive(Subcommand)]
pub enum GovCommand {
    Propose { patch: PathBuf },
    Shadow { id: String },
    Approve { id: String, decision: Option<String> },
    Apply { id: String },
    List { status: Option<String> },
    Show { id: String },
}
```

### World Resolution Helper

```rust
fn resolve_world(opts: &WorldOpts) -> Result<PathBuf, CliError> {
    if let Some(w) = &opts.world {
        return Ok(w.clone());
    }
    if let Ok(w) = std::env::var("AOS_WORLD") {
        return Ok(PathBuf::from(w));
    }
    let cwd = std::env::current_dir()?;
    if cwd.join("air").exists() || cwd.join(".aos").exists() || cwd.join("manifest.air.json").exists() {
        return Ok(cwd);
    }
    Err(CliError::NoWorldSpecified)
}
```

### Daemon Detection

```rust
async fn try_control_client(store_root: &Path) -> Option<ControlClient> {
    let socket_path = store_root.join(".aos/control.sock");
    if socket_path.exists() {
        ControlClient::connect(&socket_path).await.ok()
    } else {
        None
    }
}
```

## Tasks

- [x] Refactor `WorldOpts` as global Clap args with env var support.
- [x] Implement world resolution helper with CWD detection.
- [x] Remove `step` command; update `run` with `--batch` flag.
- [x] Add new commands: `info`, `event`, `state`, `head`, `manifest`, `put-blob`, `shutdown`.
- [x] Add `world gov` subcommand tree (propose/shadow/approve/apply/list/show) as stubs.
- [x] Wire all control-surface commands through `ControlClient` when daemon is present.
- [x] Add batch-mode fallback for commands that can operate without a daemon.
- [x] Add file/stdin input helpers for `@file` and `@-` syntax.
- [x] Add `--raw` output formatting for `state` command (pretty by default).
- [ ] Update documentation and help text.

## Success Criteria

All verified:

- [x] `export AOS_WORLD=./examples/00-counter && aos world run --batch` processes until quiescent.
- [x] `aos world event demo/CounterEvent@1 '{"Start":{"target":3}}' --batch` enqueues event and runs until quiescent.
- [x] `aos world event demo/CounterEvent@1 '"Tick"'` (no --batch) writes to journal if no daemon.
- [x] `aos world state demo/CounterSM@1` returns formatted JSON (pretty by default).
- [x] `aos world gov list` prints stub message (governance not yet implemented).
- [x] Commands detect running daemon and use control channel; fall back to batch mode when appropriate.
- [x] No REPL code; all interaction is through CLI subcommands.

## Future Extensions

- `aos world logs` / `aos world tail` — live journal streaming when control protocol supports it.
- `aos world replay` — replay journal for debugging/testing.
- `aos world dev` — convenience wrapper that starts daemon and provides enhanced feedback.
- Interactive terminal UI (Ink-style) if we need richer live interaction.
