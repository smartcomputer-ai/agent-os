# P3: AOS Host Adapter Provider

**Priority**: P3  
**Status**: Complete (first cut)  
**Effort**: High  
**Risk if deferred**: High (Fabric would remain a standalone control plane that AOS cannot use
through the existing `host.*` effect surface)  
**Depends on**:
- `roadmap/v0.20-fabric/fabric.md`
- `roadmap/v0.20-fabric/p1-fabric-host-daemon.md`
- `roadmap/v0.20-fabric/p2-fabric-controller.md`

## Goal

Make Fabric usable by AOS as a backend for the existing `host.*` effects.

P3 should prove that `aos-effect-adapters` can:

1. extend AOS runtime/adapter plumbing enough to support Fabric properly,
2. add Fabric-backed provider routes for the same `host.*` effect kinds,
3. open a Fabric sandbox session through the controller,
4. run `host.exec` through the controller with time-based progress frames for long executions,
5. translate Fabric filesystem RPCs into the existing `sys/Host*` receipt shapes,
6. use AOS intent idempotency as Fabric controller request IDs,
7. keep Fabric crates free of AOS dependencies.

P3 is the AOS adapter integration phase. The Fabric integration may refactor host adapters and async
effect startup where needed, but Fabric crates themselves must stay independent from AOS runtime
crates.

## Current Code Facts

### Fabric Side

The Fabric side already exposes the controller-facing client surface P3 needs:

- `crates/fabric-client/src/controller.rs` has `FabricControllerClient`.
- `open_session` calls `POST /v1/sessions`.
- `exec_session_stream` calls `POST /v1/sessions/{session_id}/exec` and decodes NDJSON
  `ExecEvent`s.
- `signal_session` calls `POST /v1/sessions/{session_id}/signal`.
- filesystem methods proxy through the controller to the assigned host.

The protocol already has:

- `ControllerSessionOpenRequest` with `request_id`, tagged `FabricSessionTarget`, TTL, and labels,
- `FabricSessionTarget::{Sandbox, AttachedHost}`,
- `ControllerExecRequest` with `request_id`, argv, cwd, env patch, stdin, and timeout,
- `FabricSessionSignal` tagged variants,
- idempotent controller session opens and execs when `request_id` is supplied.

Known Fabric gaps that affect P3:

- controller auth exists in the client shape but P2 intentionally left bearer-token enforcement open,
- Fabric protocol now carries binary-safe payloads through `FabricBytes::{Text, Base64}`,
- exec stdin supports text and base64 through the controller and host protocol,
- exec stdout/stderr events carry binary-safe `data` instead of lossy text,
- filesystem read/write protocol uses `content: FabricBytes`,
- Fabric fs read/stat responses include `mtime_ns` where available,
- Fabric fs `list_dir`, `grep`, and `glob` return structured JSON that the AOS adapter must
  render into the current text-output receipt fields.

### AOS Side

The current AOS host adapter is monolithic:

- `crates/aos-effect-adapters/src/adapters/host/mod.rs`
- local process/session state lives in `HostState`,
- every `host.*` adapter decodes CBOR params, performs local work, materializes output into the
  AOS Store/CAS, and builds a final `EffectReceipt`,
- the default registry registers one adapter per `host.*` effect kind,
- `EffectAdapterConfig.adapter_routes` maps logical adapter IDs to adapter provider kinds.

Current default local routes include:

```text
host.session.open.default -> host.session.open
host.exec.default -> host.exec
host.session.signal.default -> host.session.signal
host.fs.read_file.default -> host.fs.read_file
...
```

The async adapter trait already supports streaming updates:

```text
AsyncEffectAdapter::ensure_started(intent, updates)
EffectUpdate::StreamFrame(EffectStreamFrame)
EffectUpdate::Receipt(EffectReceipt)
```

The adapter/runtime path now has an `AdapterStartContext` for origin metadata required for accepted
stream frames:

- `origin_module_id`,
- `origin_instance_key`,
- `effect_kind`,
- `emitted_at_seq`.

The kernel validates those fields against the in-flight workflow effect context. A Fabric exec
adapter that emits stream frames without identity can rely on the node runtime to fill the frame
identity from the startup context before admission.

Completed AOS schema and runtime changes:

- `crates/aos-effect-types/src/host.rs` supports `HostTarget::Local` and
  `HostTarget::Sandbox`.
- `spec/defs/builtin-schemas-host.air.json` includes the `sandbox` HostTarget variant.
- `crates/aos-sys/src/bin/cap_enforce_host.rs` only understands local host targets, but
  full host capability policy is explicitly deferred out of P3.
- adapter startup receives effect-origin metadata for workflow-origin async effects and normalizes
  stream-frame identity before kernel admission.
- the host adapter implementation is split into local, Fabric, shared, output, path, state, and
  patch modules.

## Repository Model

Fabric lives in this AOS workspace. `exa-fac` is downstream factory code and should consume Fabric
through AOS APIs or binaries built from this repo.

The dependency direction should be:

```text
aos-effect-adapters
  -> fabric-client
  -> fabric-protocol
```

The forbidden direction is:

```text
fabric-* -> aos-node / aos-kernel / aos-effect-adapters
```

`aos-effect-adapters` depends on the in-workspace Fabric client and protocol crates:

```toml
fabric-client = { path = "../fabric-client" }
fabric-protocol = { path = "../fabric-protocol" }
```

Do not reintroduce dependencies from AOS back into `/Users/lukas/dev/exa-fac`.

## Non-Goals

P3 does not implement:

- bearer-token enforcement in the Fabric controller or host,
- attached-host execution,
- SSH sessions,
- Kubernetes scheduling,
- permanent app deployment,
- multi-controller replication,
- a new `fabric.*` AOS effect catalog,
- direct AOS adapter calls to Fabric host daemons,
- final host capability and policy enforcement for sandbox targets.

## Design Rules

### 1) Keep `host.*` As The AOS API

AOS workflows should continue to emit:

- `host.session.open`
- `host.exec`
- `host.session.signal`
- `host.fs.read_file`
- `host.fs.write_file`
- `host.fs.edit_file`
- `host.fs.apply_patch`
- `host.fs.grep`
- `host.fs.glob`
- `host.fs.stat`
- `host.fs.exists`
- `host.fs.list_dir`

Fabric-backed adapter kinds should be provider implementations, not new workflow-visible effect
kinds:

```text
host.session.open.fabric
host.exec.fabric
host.session.signal.fabric
host.fs.read_file.fabric
host.fs.write_file.fabric
host.fs.edit_file.fabric
host.fs.apply_patch.fabric
host.fs.grep.fabric
host.fs.glob.fabric
host.fs.stat.fabric
host.fs.exists.fabric
host.fs.list_dir.fabric
```

World manifests bind `host.*` effect kinds to logical adapter IDs. Host configuration maps those
logical adapter IDs to either local or Fabric provider kinds.

Example:

```text
manifest.effect_bindings:
  host.session.open -> host.session.open.sandbox
  host.exec -> host.exec.sandbox
  host.fs.read_file -> host.fs.read_file.sandbox

AOS_ADAPTER_ROUTES:
  host.session.open.sandbox=host.session.open.fabric,
  host.exec.sandbox=host.exec.fabric,
  host.fs.read_file.sandbox=host.fs.read_file.fabric
```

### 2) Split Host Adapter Wrappers From Host Backends

Refactor `aos-effect-adapters/src/adapters/host` into shared wrappers and backend
implementations.

Suggested shape:

```text
adapters/host/
  mod.rs
  backend.rs
  local.rs
  fabric.rs
  output.rs
  patch/
  paths.rs
  state.rs
```

The shared wrappers should own:

- CBOR param decode,
- normalized legacy shape handling for existing host params,
- AOS Store/CAS output materialization,
- `EffectReceipt` construction,
- `EffectStreamFrame` construction,
- adapter provider kind strings,
- common error-to-receipt mapping.

Backends should own:

- session open/lookup/signal,
- exec execution and output observation,
- filesystem operations,
- provider-specific state and clients.

The local backend should keep the useful current process/session behavior, but P3 does not need to
preserve the exact internal structure or test helper API. Existing tests may be rewritten or
replaced if the new backend boundary is cleaner.

### 3) Add AOS Host Target Variants

Extend the AOS host target type and built-in schema with a sandbox variant.

Suggested Rust shape:

```rust
pub struct HostSandboxTarget {
    pub image: String,
    #[serde(default)]
    pub runtime_class: Option<String>,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub network_mode: Option<String>,
    #[serde(default)]
    pub mounts: Option<Vec<HostMount>>,
    #[serde(default)]
    pub cpu_limit_millis: Option<u64>,
    #[serde(default)]
    pub memory_limit_bytes: Option<u64>,
}

#[serde(tag = "$tag", content = "$value", rename_all = "snake_case")]
pub enum HostTarget {
    Local(HostLocalTarget),
    Sandbox(HostSandboxTarget),
}
```

Keep `session_ttl_ns` and `labels` on `HostSessionOpenParams`; do not duplicate them inside
`HostSandboxTarget`.

Local backend behavior:

- accepts `local`,
- rejects `sandbox` with `unsupported_target`.

Fabric backend behavior for P3:

- accepts `sandbox`,
- rejects `local` with `unsupported_target` unless a future explicit Fabric local proxy mode exists.

The built-in AIR schema `sys/HostTarget@1` must gain the `sandbox` variant. Add schema and
normalization tests so existing `local` CBOR remains stable.

### 4) Defer Host Capability Policy

P3 should not spend design time on final host capability policy.

Minimum P3 requirement:

- AOS crates still compile after adding `HostTarget::Sandbox`,
- schema normalization accepts the new target,
- tests that exercise the old local-only policy are updated or temporarily scoped to local targets,
- sandbox policy decisions are documented as deferred.

It is acceptable for P3 to make sandbox host effects allowed in development configurations while
policy is being rebuilt. Capability enforcement for `allowed_targets`, network modes, mounts, env,
TTL, image allowlists, and resource limits will be handled in a later security/policy phase.

### 5) Expand Fabric Protocol For Binary Data

P3 should extend Fabric before or alongside the AOS adapter so the adapter does not need text-only
fallbacks for normal host effects.

Required protocol changes:

- support binary exec stdin end to end,
- support binary file read responses,
- support binary file write requests,
- preserve text ergonomics for CLI and JSON debugging,
- include enough file metadata to fill AOS receipt fields where practical, especially `mtime_ns`.

Suggested shape:

```rust
#[serde(tag = "kind", content = "spec", rename_all = "snake_case")]
pub enum FabricBytes {
    Text(String),
    Base64(String),
}

pub struct FsFileReadResponse {
    pub path: String,
    pub content: FabricBytes,
    pub offset_bytes: u64,
    pub bytes_read: u64,
    pub size_bytes: u64,
    pub truncated: bool,
    pub mtime_ns: Option<u128>,
}

pub struct FsFileWriteRequest {
    pub path: String,
    pub content: FabricBytes,
    #[serde(default)]
    pub create_parents: bool,
}
```

`ExecStdin::Base64` should be decoded by the controller or forwarded losslessly to the host after
the host protocol can carry binary stdin. The controller should stop returning `unsupported_stdin`
for valid base64 stdin once this lands.

Compatibility can be handled in the CLI/client layer with convenience helpers for `text`, but the
protocol should no longer be text-only.

### 6) Fabric Backend Config

Add Fabric config under `EffectAdapterConfig`.

Suggested Rust shape:

```rust
pub struct FabricAdapterConfig {
    pub controller_url: String,
    pub bearer_token: Option<String>,
    pub request_timeout: Duration,
    pub exec_progress_interval: Duration,
    pub default_image: Option<String>,
    pub default_runtime_class: Option<String>,
    pub default_network_mode: Option<String>,
}
```

Suggested env vars:

```text
AOS_FABRIC_CONTROLLER_URL=http://127.0.0.1:8787
AOS_FABRIC_BEARER_TOKEN=...
AOS_FABRIC_BEARER_TOKEN_FILE=...
AOS_FABRIC_REQUEST_TIMEOUT_SECS=300
AOS_FABRIC_EXEC_PROGRESS_INTERVAL_SECS=10
AOS_FABRIC_DEFAULT_IMAGE=docker.io/library/alpine:latest
AOS_FABRIC_DEFAULT_RUNTIME_CLASS=smolvm
AOS_FABRIC_DEFAULT_NETWORK_MODE=egress
```

If both token env vars are present, the explicit token wins. Token file loading should trim
surrounding whitespace.

Do not register Fabric provider adapters unless `controller_url` is configured, or register them as
stub adapters that return a stable `fabric_not_configured` receipt. The preferred behavior is to
avoid route registration and let route preflight catch accidental Fabric bindings.

### 7) Idempotency Mapping

Map AOS intent idempotency to Fabric controller request IDs.

Use a stable text request ID:

```text
aos:{hex(intent.intent_hash)}
```

This should be used for:

- `ControllerSessionOpenRequest.request_id`,
- `ControllerExecRequest.request_id`.

Do not use random request IDs from the adapter. The controller already uses request IDs for replay
and conflict detection.

Filesystem and signal RPCs are not idempotent in P2. P3 should not invent controller-side
idempotency for them. If retried by AOS, they can re-run unless a later Fabric protocol revision
adds request IDs to those operations.

### 8) Session IDs

AOS receipts should expose the controller session ID returned by Fabric:

```text
HostSessionOpenReceipt.session_id = ControllerSessionOpenResponse.session_id
```

The Fabric backend should pass that same ID back to controller operations. The adapter must not
translate it into a host-local session ID.

### 9) Exec Streaming

P3 must fix the adapter/runtime metadata gap before claiming accepted stream frames.

Preferred runtime change:

```rust
pub struct AdapterStartContext {
    pub origin_module_id: String,
    pub origin_instance_key: Option<Vec<u8>>,
    pub effect_kind: String,
    pub emitted_at_seq: u64,
}

async fn ensure_started_with_context(
    &self,
    intent: EffectIntent,
    context: AdapterStartContext,
    updates: EffectUpdateSender,
) -> anyhow::Result<()>;
```

`OpenedEffect` already carries `EffectIntentRecord`, and that record carries origin metadata. The
node/runtime should pass that metadata to adapters instead of dropping it when it materializes an
`EffectIntent`.

The AOS adapter must not translate every Fabric `ExecEvent` into an AOS stream frame. Fabric's
NDJSON stream is an internal observation channel between the controller and adapter. The AOS-facing
stream should be time-based progress checkpoints.

Default checkpoint policy:

- continuously consume Fabric NDJSON events so the HTTP stream does not back up,
- aggregate stdout and stderr separately in adapter memory, spilling to Store/CAS later if needed,
- emit no AOS stream frame before the first progress interval elapses,
- use a default progress interval of 10 seconds, configurable through
  `AOS_FABRIC_EXEC_PROGRESS_INTERVAL_SECS`,
- while the command is still running, emit one progress frame per elapsed interval,
- if the command finishes before the first interval, emit no progress frames and only return the
  terminal receipt,
- at completion, always return a terminal `HostExecReceipt` containing the entire exec result,
- do not emit a duplicate final progress frame just because the command completed.

Fabric `ExecEvent` handling:

- `started`: record exec identity and timing; no immediate AOS frame,
- `stdout`: append text to stdout aggregation and to the current progress-window delta,
- `stderr`: append text to stderr aggregation and to the current progress-window delta,
- `exit`: stop streaming and return terminal receipt with `ReceiptStatus::Ok` when exit code is
  present,
- `error`: stop streaming and return terminal receipt with `ReceiptStatus::Error`.

The time-based aggregation is an adapter responsibility, but most of the generic stream mechanics
can live in `fabric-client` so they are tested on the Fabric side and reused by any future client.
`fabric-client` should provide utilities that consume `ExecEventClientStream` and produce:

- periodic progress snapshots/deltas based on a caller-supplied interval,
- complete stdout/stderr aggregation,
- terminal exit/error metadata,
- deterministic behavior for commands that finish before the first interval.

Those utilities must remain AOS-neutral. They should not construct `EffectStreamFrame`s or depend on
AOS crates.

Add a host exec progress payload schema, for example:

```rust
pub struct HostExecProgressFrame {
    pub exec_id: Option<String>,
    pub elapsed_ns: u64,
    pub stdout_delta: Vec<u8>,
    pub stderr_delta: Vec<u8>,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
}
```

Suggested `EffectStreamFrame.kind` value:

```text
host.exec.progress
```

Frame `seq` should be strictly monotonic starting at 1 for each emitted progress frame. Do not
forward Fabric event seq blindly; normalize to the AOS stream cursor contract.

The final `HostExecReceipt` should still include materialized stdout/stderr using current AOS
output rules and should contain the complete result, including output already reported in progress
frames. Progress frames are periodic execution updates, not a replacement for the terminal receipt.

### 10) Output And Input Mapping

Exec params:

| AOS field | Fabric field |
| --- | --- |
| `session_id` | URL session ID |
| `argv` | `ControllerExecRequest.argv` |
| `cwd` | `ControllerExecRequest.cwd` |
| `env_patch` | `ControllerExecRequest.env_patch` |
| `timeout_ns` | `ControllerExecRequest.timeout_ns` |
| `stdin_ref` | `ControllerExecRequest.stdin` |

For `stdin_ref`, P3 should read the blob from the AOS Store:

- if bytes are UTF-8, it may send `ExecStdin::Text`,
- otherwise send `ExecStdin::Base64`,
- never corrupt non-UTF-8 stdin by lossy conversion.

Exec outputs:

- aggregate Fabric stdout/stderr text chunks separately,
- materialize through existing `materialize_output`,
- respect `output_mode = auto | require_inline`,
- reject unknown output modes exactly like the local backend.
- progress frames may include window deltas inline; the terminal receipt remains the authoritative
  complete output.

Filesystem read:

- call `FabricControllerClient::read_file`,
- map returned text/base64 bytes to `HostOutput::InlineText`, `HostOutput::InlineBytes`, or blob
  according to output mode, requested encoding, and size,
- set `status = ok`,
- set `truncated` and `size_bytes`,
- set `mtime_ns` when Fabric returns it.

Filesystem write:

- support `HostFileContentInput::InlineText`,
- support `HostFileContentInput::InlineBytes`,
- support blob input by reading bytes from the AOS Store and sending text or base64 as appropriate,
- map Fabric `bytes_written` to `written_bytes`.

Filesystem edit/apply_patch:

- pass text requests directly to Fabric,
- map Fabric counters into existing AOS receipts,
- preserve dry-run behavior for apply_patch.

Filesystem grep/glob/list_dir:

- render Fabric structured results into newline-delimited text to match existing AOS receipts,
- materialize through `materialize_text_output`,
- preserve `count`, `match_count`, and `truncated`.

Filesystem stat/exists:

- map missing/not-found responses to current AOS status conventions where possible,
- map `FsEntryKind::Directory` to `is_dir = true`,
- map other existing kinds to `is_dir = false`,
- set `mtime_ns` when Fabric exposes it.

### 11) Error Mapping

Fabric client errors should become typed AOS receipts, not adapter task failures, whenever possible.

Suggested mapping:

| Fabric error code/status | AOS receipt status | AOS error_code |
| --- | --- | --- |
| `unsupported_target` | Error | `unsupported_target` |
| `unsupported_lifecycle` | Error | `unsupported_lifecycle` |
| `no_healthy_host` | Error | `no_healthy_host` |
| `host_error` | Error | `fabric_host_error` |
| `not_found` | Ok or Error by effect convention | `not_found` when the existing receipt type uses it |
| HTTP/network failure | Error | `fabric_unreachable` |
| invalid adapter config | Error | `fabric_config_invalid` |

Do not panic or let adapter tasks end without sending a receipt. If `ensure_started` returns without
a terminal receipt, the runtime will synthesize a generic adapter-start error; P3 should avoid that
for normal Fabric failures.

## Implementation Plan

### Step 1: Expand Fabric Protocol And Client Helpers

Status:

- Completed in Fabric: binary-safe `FabricBytes`, binary exec stdin/events, binary file read/write,
  file `mtime_ns`, OpenAPI components, CLI raw output handling, and host/controller test coverage.
- Completed in `fabric-client`: AOS-neutral exec aggregation/progress utilities with byte-safe
  stdout/stderr aggregation, interval-based progress deltas, terminal exit/error metadata, and
  deterministic fast-exec behavior.

- Add binary stdin support through controller and host protocol.
- Add binary file read/write support.
- Add file metadata needed by AOS receipts, especially `mtime_ns`.
- Update `fabric-client` controller and host methods for the new content shape.
- Add AOS-neutral `fabric-client` exec aggregation/progress utilities.
- Update Fabric CLI ergonomics so text read/write remains easy.

Acceptance:

- Fabric host and controller tests cover binary stdin, binary read, binary write, and metadata,
- `fabric-client` progress utility tests cover no-progress-before-interval, periodic progress, and
  complete terminal aggregation,
- controller fake-host tests use the new protocol shape.

### Step 2: Extend AOS Runtime And Host Schemas

Status:

- Completed first cut: AOS `HostTarget` now supports `local` and `sandbox`, with built-in schema
  coverage and local adapter rejection for sandbox targets.
- Completed first cut: async adapter startup can carry `AdapterStartContext`; node runtime passes
  workflow effect-origin metadata from pending/opened effects and normalizes stream-frame identity
  before forwarding frames to the kernel.

- Add `HostSandboxTarget` to `aos-effect-types`.
- Add the `sandbox` variant to `HostTarget`.
- Update helper constructors and tests.
- Update `builtin-schemas-host.air.json`.
- Update `aos-air-types` built-in schema tests.
- Thread effect origin metadata from opened effect records into adapter startup.
- Update or temporarily scope local-only capability tests; do not design final sandbox policy here.

Acceptance:

- new schema tests prove `local` and `sandbox` encode/decode correctly,
- async adapter startup can receive the origin metadata required for accepted stream frames,
- AOS crates compile without final sandbox capability policy.

### Step 3: Refactor Host Adapters Behind A Backend Boundary

Status:

- Completed cleanup cut in AOS: `adapters/host/mod.rs` is now a thin module boundary.
- Completed cleanup cut in AOS: the existing local implementation moved to `adapters/host/local.rs`.
- Completed cleanup cut in AOS: shared receipt construction, normalized param decoding, blob-backed
  file content resolution, and patch text resolution moved to `adapters/host/shared.rs`.
- Completed cleanup cut in AOS: local and Fabric adapters are peer modules that share helper code
  without Fabric importing local backend internals.

- [x] Introduce a `HostBackend` trait or equivalent backend boundary.
- [x] Move current local state/process/fs logic behind the local host module boundary.
- [x] Keep shared receipt, output, and stream-frame helpers outside Fabric-specific code.
- [x] Preserve local host tests around the new boundary.

Acceptance:

- local backend behavior needed by development still works,
- no Fabric controller is required for local backend tests,
- the new boundary is clean enough for `FabricHostBackend` without special cases.

### Step 4: Add Fabric Adapter Config And Registration

Status:

- Completed first cut: `FabricAdapterConfig` parses controller URL, bearer token, request timeout,
  exec progress interval, and default sandbox fields from env/config.
- Completed first cut: configured Fabric controller URL registers terminal provider kinds for
  `host.session.open.fabric`, `host.exec.fabric`, `host.session.signal.fabric`,
  `host.fs.read_file.fabric`, `host.fs.write_file.fabric`, `host.fs.edit_file.fabric`,
  `host.fs.apply_patch.fabric`, `host.fs.grep.fabric`, `host.fs.glob.fabric`,
  `host.fs.stat.fabric`, `host.fs.exists.fabric`, and `host.fs.list_dir.fabric`.

- Add `FabricAdapterConfig` to `EffectAdapterConfig`.
- Parse Fabric env vars.
- Add `make_fabric_host_adapter_set` or equivalent.
- Register Fabric provider kinds only when configured.
- Add route tests for `AOS_ADAPTER_ROUTES` mapping Fabric logical routes to Fabric provider kinds.

Acceptance:

- default config preserves existing local routes,
- configured Fabric controller URL registers `host.*.fabric` provider kinds,
- missing Fabric config produces clear route diagnostics or stable adapter errors.

### Step 5: Implement `FabricHostBackend`

Status:

- Completed first cut: terminal Fabric adapters open sandbox sessions, run exec through controller
  NDJSON streaming, signal sessions, and read/write files through controller RPCs.
- Completed first cut: open and exec use `aos:{hex(intent.intent_hash)}` as Fabric request IDs.
- Completed first cut: exec and fs output use existing AOS output materialization rules, including
  byte-safe inline/blob handling.
- Completed first cut: read/write/grep/glob/stat/exists/list_dir map Fabric controller RPCs into
  current AOS receipt shapes, including newline-delimited text materialization for structured
  search/list results.
- Completed first cut: edit/apply_patch map Fabric controller RPCs into current AOS receipt
  shapes.
- Completed cleanup cut: factored the host adapter module into local, Fabric, shared, output, path,
  state, and patch modules with a thin public boundary.

- [x] Convert `HostSessionOpenParams` sandbox target to `ControllerSessionOpenRequest`.
- [x] Convert `HostExecParams` to `ControllerExecRequest`.
- [x] Convert `HostSessionSignalParams` to `ControllerSignalSessionRequest`.
- [x] Convert filesystem params/receipts.
- [x] Use `FabricControllerClient`, not raw `reqwest`, except where client gaps are discovered.

Acceptance:

- adapter unit tests with a fake Fabric controller cover open, exec, signal, read, write, grep,
  glob, stat, exists, and list_dir,
- idempotent open/exec tests prove repeated AOS intents use the same Fabric request ID.

### Step 6: Implement AOS Exec Progress Streaming

Status:

- Completed first cut: `sys/HostExecProgressFrame@1` is in AOS built-ins.
- Completed first cut: `host.exec.fabric` emits time-based `host.exec.progress` stream frames from
  `fabric-client` progress snapshots when async startup provides origin context.
- Completed first cut: adapter tests cover a long exec that emits progress before the terminal
  receipt and a fast exec that emits only the terminal receipt.
- Completed first cut: embedded hosted-worker test uses a fake Fabric controller with a real AOS
  world/runtime/kernel/journal path and proves `host.exec.progress` is admitted before the terminal
  receipt without `stream.identity_mismatch`.
- Completed first cut: gated live e2e exists for a real Fabric controller plus smolvm host.

- [x] Use `fabric-client` aggregation utilities to consume Fabric exec observations.
- [x] Build valid time-based `EffectStreamFrame`s from aggregated progress updates.
- [x] Add a `HostExecProgressFrame` payload type/schema.
- [x] Keep final terminal receipt aggregation.

Acceptance:

- a node/kernel integration test admits progress frames for an exec that runs longer than the
  configured interval,
- an exec that finishes before the first interval emits no stream frames and returns only the
  terminal receipt,
- stream frames are not dropped as `stream.identity_mismatch`,
- Fabric event seq is not exposed directly as AOS stream frame seq,
- the terminal receipt contains the complete stdout/stderr result.

### Step 7: Add Gated Fabric E2E

Status:

- Completed first cut: added `adapters_host_fabric_live`, a gated AOS adapter live e2e that is
  skipped unless `AOS_FABRIC_E2E=1`.
- The test assumes an already running Fabric controller with at least one registered smolvm-capable
  host, then exercises sandbox open, text/binary fs read/write, exists/stat/list_dir/grep/glob,
  edit/apply_patch, binary-stdin exec, progress-frame exec, and session close.
- Completed live run against local Fabric infrastructure with `AOS_FABRIC_CONTROLLER_URL` set to
  `http://127.0.0.1:8788`.

Add a gated e2e test in AOS that assumes an already running Fabric controller and at least one
registered smolvm-capable `fabric-host`.

Suggested env vars:

```text
AOS_FABRIC_E2E=1
AOS_FABRIC_CONTROLLER_URL=http://127.0.0.1:8787
AOS_FABRIC_IMAGE=docker.io/library/alpine:latest
AOS_FABRIC_EXEC_PROGRESS_INTERVAL_SECS=1
```

Current local run command from the AOS repo root:

```sh
AOS_FABRIC_E2E=1 \
AOS_FABRIC_CONTROLLER_URL=http://127.0.0.1:8788 \
AOS_FABRIC_IMAGE=alpine:latest \
AOS_FABRIC_EXEC_PROGRESS_INTERVAL_SECS=1 \
cargo test -p aos-effect-adapters --test adapters_host_fabric_live -- --nocapture
```

The test now:

1. opens a sandbox session,
2. writes and reads a text file,
3. writes and reads a binary file,
4. execs a command that receives binary stdin,
5. runs grep/glob/list_dir,
6. signals close,
7. verifies terminal receipts,
8. runs one long exec with a short configured progress interval and verifies at least one progress
   frame before the terminal receipt.

Acceptance:

- skipped by default unless the e2e env var is set,
- documented command sequence starts controller plus host from this repo and then runs the AOS
  test from this repo.

## Open Decisions

### Capability Policy

Final host capability policy is explicitly deferred. P3 should make sandbox targets usable in
development and leave a clear follow-up for policy enforcement across targets, images, network
modes, mounts, env, TTLs, and resource hints.

### Adapter Dependency Form

Fabric client and protocol crates are in-workspace AOS crates. Keep the dependency direction as
`aos-effect-adapters -> fabric-client -> fabric-protocol`; do not add `aos-*` dependencies to
Fabric crates.

### Stream Payload Schema

If existing workflows only need generic `EffectStreamFrame` envelopes, P3 can keep the
`HostExecProgressFrame` payload schema small. If workflows need strongly typed host progress
events, add `sys/HostExecProgressFrame@1` to built-ins and include it in manifest examples.

## Completion Checklist

- [x] AOS `HostTarget` supports `local` and `sandbox`.
- [x] AOS async adapter startup carries origin metadata needed for accepted stream frames.
- [x] Fabric protocol supports binary exec stdin and binary file read/write.
- [x] `fabric-client` has tested exec aggregation/progress utilities.
- [x] Host adapters are split into shared wrappers plus local backend.
- [x] Local host adapter tests are rewritten or preserved around the new backend boundary.
- [x] Fabric adapter config is parsed from env/config.
- [x] Fabric provider kinds are registered through `EffectAdapterConfig.adapter_routes`.
- [x] `FabricHostBackend` opens sandbox sessions through the controller.
- [x] `FabricHostBackend` runs exec through controller NDJSON streaming.
- [x] Fabric exec adapter emits time-based progress frames before terminal receipt.
- [x] Exec progress frames are admitted by a node/kernel workflow integration test.
- [x] Execs that finish before the first progress interval emit no stream frames.
- [x] Final exec receipts aggregate stdout/stderr using existing AOS output rules.
- [x] Fabric read/write/grep/glob/stat/exists/list_dir map to current AOS receipt shapes.
- [x] Fabric edit/apply_patch map to current AOS receipt shapes.
- [x] AOS intent hash is used as Fabric controller request ID for open and exec.
- [x] Gated AOS Fabric e2e test passes against a real controller plus smolvm host.
- [x] Sandbox host capability policy is documented as deferred.
