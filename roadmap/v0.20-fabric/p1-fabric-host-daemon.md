# P1: Fabric Host Daemon With Smolvm Sessions

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (the AOS adapter and controller would otherwise be designed against an
unproven execution substrate)  
**Status**: Proposed  
**Depends on**:
- `roadmap/v0.20-fabric/fabric.md`
- `roadmap/v0.20-fabric/rec.md`

## Goal

Build the first standalone fabric host daemon.

P1 should prove that one installed daemon on one Unix host can:

1. create isolated smolvm-backed sessions from OCI images,
2. run parallel RPC-style exec calls inside a session,
3. stream stdout/stderr while commands run,
4. expose confined filesystem operations against the session workspace,
5. quiesce, resume, and close sessions,
6. reconstruct live/quiesced sessions from smolvm runtime inventory after daemon restart,
7. be exercised directly with `curl` or a small CLI before any controller or AOS adapter exists.

This phase intentionally starts outside AOS. There is no `aos-effect-adapters` work in P1 and no
controller scheduling layer. P1 is the host substrate that P2 and P3 will build on.

## Why This Exists

Fabric should not begin with an adapter shim that pretends remote execution exists.

The hard part is proving the operational substrate:

1. VM/session lifecycle,
2. streaming behavior,
3. filesystem confinement,
4. storage roots and cleanup,
5. restart inventory without a daemon-owned database,
6. resource and security defaults.

Once the host daemon is real, the controller can schedule hosts and the AOS adapter can translate
`host.*` effects into fabric RPCs. If the host daemon is not proven first, both later layers will
encode guesses about what the backend can actually do.

## Non-Goals

P1 does not implement:

- the fabric controller,
- host registration or heartbeat to a controller,
- AOS effect adapter routes,
- `HostTarget` schema changes,
- durable AOS replay tests,
- non-smolvm runtime backends,
- SSH sessions,
- Kubernetes-managed session pods,
- permanent service deployment,
- shared volumes across sessions,
- cross-session networking,
- Docker inside sessions,
- Docker/Podman runtime support,
- queued job semantics.

## Crate Shape

Add a new standalone workspace crate:

```text
crates/aos-fabric/
  Cargo.toml
  src/lib.rs
  src/protocol.rs
  src/host/config.rs
  src/host/http.rs
  src/host/state.rs
  src/host/service.rs
  src/host/fs.rs
  src/host/exec.rs
  src/host/smolvm.rs
  src/bin/aos-fabric-host.rs
  tests/host_daemon_smoke.rs
```

`aos-fabric` should not depend on `aos-node`, `aos-kernel`, or `aos-effect-adapters`.

Acceptable dependencies:

- async/runtime and HTTP crates,
- serialization crates,
- `smolvm` as the only runtime dependency,
- common utility crates.

Use the smolvm Rust API directly. P1 should not shell out to the `smolvm` CLI and should not call
the smolvm HTTP server as a sidecar.

### Smolvm Dependency Strategy

Use a pinned smolvm source dependency for P1.

Recommended initial shape:

```text
third_party/
  smolvm/   # git submodule: https://github.com/smol-machines/smolvm.git
```

Reference the crates directly from `crates/aos-fabric/Cargo.toml`:

```toml
[dependencies]
smolvm = { path = "../../third_party/smolvm" }
smolvm-protocol = { path = "../../third_party/smolvm/crates/smolvm-protocol" }
```

Do not add `third_party/smolvm` as an AOS workspace member. Treat it as an external source
dependency. This keeps the AOS workspace boundary clear while still allowing P1 to pin and patch
smolvm while the integration surface is settling.

Do not commit dependencies that point at a developer-local path such as
`/Users/lukas/dev/tmp/smolvm`. A local sibling checkout is acceptable for investigation only.

If smolvm no longer needs local patches, the dependency can move from submodule path dependencies
to pinned git dependencies:

```toml
[dependencies]
smolvm = { git = "https://github.com/smol-machines/smolvm.git", rev = "<pinned-sha>" }
smolvm-protocol = { git = "https://github.com/smol-machines/smolvm.git", package = "smolvm-protocol", rev = "<pinned-sha>" }
```

If smolvm crates are published later, prefer normal crates.io version dependencies.

Prefer keeping protocol types in `src/protocol.rs` for P1. If the protocol stabilizes, P2 can
split it into a smaller shared crate.

## Operating Model

P1 runs in direct-host mode:

```text
client or curl
  |
  HTTP JSON / NDJSON
  |
aos-fabric-host
  |
smolvm facade
  |
smolvm / libkrun / Hypervisor.framework or KVM
```

The daemon listens on loopback by default. A later controller will call the same host API with
authentication and lease metadata.

P1 should use smolvm because it already provides the first runtime shape fabric wants:

- OCI images without a Docker daemon,
- one lightweight VM per workload,
- persistent machines that can be stopped, started, and exec'd into,
- network disabled by default with explicit egress controls,
- host directory mounts for the session workspace,
- a Rust library/API surface that can avoid shelling out for normal operation.

## State Model

The host daemon should be stateless in the control-plane sense.

P1 should not add SQLite, RocksDB, sled, or a daemon-owned metadata database. The recoverable source
of truth is:

1. the smolvm machine inventory,
2. fabric-managed smolvm machine names,
3. per-session workspace directories.

The daemon may keep in-memory maps for active HTTP requests and active exec streams. Those maps are
ephemeral and are rebuilt from the runtime inventory after restart.

If smolvm uses its own internal machine registry or database, treat that as runtime-owned state,
not fabric control-plane state. The fabric host daemon should not add a second local database for
scheduling, idempotency, or receipt history.

The daemon still owns a local data root for session workspaces:

```text
{state_root}/
  sessions/
    {session_id}/
      workspace/
      tmp/
      logs/
```

Workspace directories are data, not daemon metadata. They can outlive machines, but a workspace
directory alone is not an active session. If a workspace exists without a managed runtime object,
inventory may report it as an orphaned workspace, not as a resumable session.

P2 will add the fabric controller and its durable state/idempotency tables. The host daemon should
not duplicate that database locally.

Required runtime identity:

- deterministic machine name: `aos-fabric-{host_id_hash}-{session_id}`
- workspace path: `{state_root}/sessions/{session_id}/workspace`
- optional marker file: `{state_root}/sessions/{session_id}/fabric-session.json`

The marker file is a recovery aid, not the authoritative record. It may contain:

- `host_id`
- `session_id`
- `machine_name`
- `image`
- `workspace_path`
- `workdir`
- `network_mode`
- `created_at_ns`
- `expires_at_ns`
- user labels such as `tenant`, `world`, or `owner`

Session status is derived from runtime inspection:

- `creating`
- `ready`
- `quiesced`
- `closing`
- `closed`
- `orphaned_workspace`
- `error`

Exec status is request-local in P1:

- `running`
- `ok`
- `error`
- `timeout`
- `canceled`

Terminal exec results are streamed to the caller and may be written to log files for debugging, but
they are not a durable idempotency record. P2 controller state is responsible for durable exec
idempotency.

## Smolvm Integration

P1 uses smolvm as the only runtime backend.

Define a narrow smolvm facade before wiring it into the host service. The facade is not an
open-ended plugin system; it is just the boundary between fabric's HTTP/session semantics and
smolvm's API details.

```rust
trait SmolvmFacade {
    async fn create_session(&self, request: CreateSessionRequest) -> Result<CreatedSession>;
    async fn start_session(&self, session: &SessionHandle) -> Result<()>;
    async fn stop_session(&self, session: &SessionHandle, grace_ns: Option<u64>) -> Result<()>;
    async fn remove_session(&self, session: &SessionHandle) -> Result<()>;
    async fn inspect_session(&self, session: &SessionHandle) -> Result<RuntimeSessionState>;
    async fn list_managed_sessions(&self, host_id: &str) -> Result<Vec<RuntimeSessionState>>;
    async fn exec_stream(&self, request: ExecRuntimeRequest) -> Result<ExecEventStream>;
}
```

P1 implementation:

- `SmolvmFacade` implemented with the smolvm Rust API directly.
- no CLI-backed runtime path.
- no smolvm HTTP-server sidecar.

Direct crate entry points from the smolvm source dependency:

- `smolvm::SmolvmDb::open_at(path)` for the smolvm-owned machine registry,
- `smolvm::{VmRecord, RecordState}` for persisted machine records,
- `smolvm::agent::{AgentManager, AgentClient, RunConfig, HostMount, VmResources}` for lifecycle
  and guest-agent RPCs,
- `smolvm::data::network::PortMapping` for port mappings,
- `smolvm_protocol::{AgentRequest, AgentResponse}` for streaming image exec when the high-level
  `AgentClient` helpers are not expressive enough.

Do not use `smolvm::embedded::EmbeddedRuntime` as the main daemon integration surface. It is useful
as a reference SDK wrapper, and `smolvm::embedded::{EmbeddedRuntime, MachineSpec}` confirms the
intended embedded direction, but it is too narrow for P1:

1. `EmbeddedRuntime::start_machine` uses the in-process launch path. The smolvm API server uses
   `AgentManager::ensure_running_via_subprocess` to avoid macOS fork-in-a-multithreaded-process
   hazards; the fabric HTTP daemon should follow that pattern.
2. `EmbeddedRuntime` caches one `AgentClient` behind a per-machine mutex, which serializes execs.
   Fabric needs multiple exec RPCs against one session, so each exec should open its own
   `AgentClient` with `manager.connect()`.
3. `EmbeddedRuntime::exec_streaming` returns `Vec<ExecEvent>` after the command completes. Fabric
   needs to emit NDJSON while the command is still running.
4. `EmbeddedRuntime::run` does not expose `RunConfig::with_persistent_overlay`, which is required
   for image-based sessions where filesystem changes should survive across exec calls.

Keep all smolvm imports contained in `src/host/smolvm.rs` so the dependency shape does not leak
through the host HTTP layer.

Boot helper requirement:

Smolvm's safe server launch path currently calls `std::env::current_exe()` and spawns:

```text
<current-exe> _boot-vm <config-path>
```

When smolvm is linked into `aos-fabric-host`, `current_exe()` is the fabric daemon binary, not the
`smolvm` CLI. P1 must handle this explicitly before using
`AgentManager::ensure_running_via_subprocess`.

Acceptable solutions:

1. Add a hidden `aos-fabric-host _boot-vm <config-path>` command that forwards to public smolvm
   boot logic.
2. Upstream a small smolvm API change so callers can configure the boot helper executable or call a
   public `boot_vm(config_path)` function.

Prefer the upstreamable smolvm API change if it is small. Do not work around this by invoking the
smolvm CLI for normal lifecycle operations.

Session creation flow:

1. Derive `machine_name = aos-fabric-{host_id_hash}-{session_id}`.
2. Open smolvm's runtime-owned database at `{state_root}/smolvm/smolvm.redb` with
   `SmolvmDb::open_at`; call `init_tables()` during daemon startup.
3. Create a `VmRecord` with:
   - `name = machine_name`,
   - one read/write host mount from `{state_root}/sessions/{session_id}/workspace` to
     `/workspace`,
   - resource fields from the session request,
   - `network`, `allowed_cidrs`, and optional port mappings from policy,
   - `image = Some(request.image)`,
   - `workdir = Some(request.workdir.unwrap_or("/workspace"))`,
   - `ephemeral = false`.
4. Insert the record with `SmolvmDb::insert_vm_if_not_exists`.
5. Build `AgentManager::for_vm_with_sizes(&machine_name, record.storage_gb, record.overlay_gb)`.
6. Start the VM from a blocking task with
   `manager.ensure_running_via_subprocess(record.host_mounts(), record.port_mappings(),
   record.vm_resources(), Default::default())`.
7. Update the smolvm record to `RecordState::Running` with the child PID and PID start time.
8. Optionally pre-pull the session image through `manager.connect()?.pull_with_registry_config`.

Image exec flow:

1. Connect a fresh `AgentClient` per exec with `manager.connect()`.
2. Convert record mounts to run mounts with `HostMount::mount_tag(index)`:
   `(tag, guest_path, read_only)`.
3. Run commands with `RunConfig::new(image, argv)`.
4. Always set:
   - `.with_workdir(Some(cwd))`,
   - `.with_mounts(record_mounts_as_run_mounts)`,
   - `.with_timeout(timeout)`,
   - `.with_persistent_overlay(Some(machine_name.clone()))`.
5. Do not use `AgentClient::vm_exec` for normal fabric commands; that runs in the VM agent rootfs,
   not in the requested OCI image.
6. Keep `vm_exec` available only for daemon-owned diagnostics or smolvm health probes.

Streaming exec flow:

The current high-level smolvm helpers are buffered:

- `AgentClient::vm_exec_streaming` collects events into a `Vec`,
- `EmbeddedRuntime::exec_streaming` also returns a `Vec`,
- there is no high-level `run_streaming` helper that combines OCI image execution,
  `persistent_overlay_id`, and caller-owned streaming.

P1 should implement a small fabric helper over the public low-level protocol:

1. send `AgentRequest::Run { interactive: true, tty: false, persistent_overlay_id: Some(machine_name),
   ... }` with `AgentClient::send_raw`,
2. wait for `AgentResponse::Started`,
3. relay `AgentResponse::Stdout` and `AgentResponse::Stderr` to NDJSON as they arrive from
   `AgentClient::recv_raw`,
4. send request stdin through `AgentRequest::Stdin` if present, then send EOF,
5. emit exactly one terminal NDJSON event on `AgentResponse::Exited` or `AgentResponse::Error`.

This uses smolvm directly while preserving fabric's streaming contract. If this turns out to need
private smolvm APIs, make a small smolvm API addition rather than falling back to shelling out.

Runtime requirements:

- never invoke a shell to construct runtime commands,
- pass argv as process arguments,
- use deterministic smolvm machine names:
  - `aos-fabric-{host_id_hash}-{session_id}`
- create/start a persistent smolvm machine for each session,
- use OCI images as the session root image,
- mount the session workspace into the VM,
- keep the VM alive between exec calls,
- run exec commands with TTY disabled so stdout/stderr can be separated,
- support CPU and memory limits,
- support `none` and `nat` network modes at minimum,
- support egress allowlists where smolvm exposes them.

The long-lived object is the smolvm machine and guest agent. Do not create a long-running idle OCI
container solely to keep the session alive; run each fabric exec against the session image using the
same persistent overlay id.

## Session Open

Endpoint:

```text
POST /v1/sessions
```

Request:

```json
{
  "request_id": "optional-idempotency-key",
  "image": "alpine:latest",
  "workdir": "/workspace",
  "env": { "KEY": "value" },
  "network_mode": "none",
  "allowed_hosts": ["registry.npmjs.org"],
  "allowed_cidrs": [],
  "cpu_limit_millis": 2000,
  "memory_limit_bytes": 4294967296,
  "ttl_ns": 86400000000000,
  "labels": { "world": "dev" }
}
```

Response:

```json
{
  "session_id": "sess_...",
  "status": "ready",
  "image": "alpine:latest",
  "workdir": "/workspace",
  "created_at_ns": 0,
  "expires_at_ns": 86400000000000
}
```

Rules:

1. `image` must match the configured image allowlist.
2. `network_mode` must match the configured network allowlist.
3. `workdir` defaults to `/workspace`.
4. If `request_id` is present, derive `session_id` deterministically from `host_id + request_id`.
5. If `request_id` is absent, generate a random `session_id`.
6. The daemon creates `{state_root}/sessions/{session_id}/workspace`.
7. The workspace is mounted read/write into the VM at `/workspace`.
8. No host control socket may be mounted.
9. SSH agent forwarding is disabled unless explicitly requested and allowed by config.
10. Session identity is encoded into the smolvm machine name and marker file before the API returns.
11. Repeating the same completed `request_id` returns the current inspection result if the managed
   machine still exists.
12. Repeating an in-flight `request_id` returns `409 request_in_flight`.
13. After the managed machine is removed, P1 does not guarantee request replay from `request_id`;
   P2 controller state owns durable idempotency.

## Exec RPC

Endpoint:

```text
POST /v1/sessions/{session_id}/exec
Accept: application/x-ndjson
```

Request:

```json
{
  "request_id": "optional-idempotency-key",
  "argv": ["bash", "-lc", "printf hello"],
  "cwd": "/workspace",
  "env_patch": { "RUST_BACKTRACE": "1" },
  "stdin_text": "optional",
  "stdin_base64": "optional",
  "timeout_ns": 30000000000
}
```

Rules:

1. `argv` must be non-empty.
2. `stdin_text` and `stdin_base64` are mutually exclusive.
3. `cwd` defaults to the session `workdir`.
4. Multiple exec calls may run against the same session concurrently.
5. A timeout kills the running exec process and emits a terminal `timeout` event.
6. The session remains usable after a command exits non-zero.
7. The terminal event is the authoritative result for that exec RPC.
8. P1 does not support reattaching to an in-flight exec stream.

For non-streaming clients, the endpoint may return one terminal JSON response. Streaming NDJSON is
the required path.

## NDJSON Exec Stream

Each line is one JSON object.

Required event kinds:

- `started`
- `stdout`
- `stderr`
- `exit`
- `error`

Common fields:

```json
{
  "exec_id": "exec_...",
  "seq": 1,
  "kind": "stdout",
  "time_ns": 0
}
```

Output event:

```json
{
  "exec_id": "exec_...",
  "seq": 2,
  "kind": "stdout",
  "time_ns": 0,
  "data_base64": "aGVsbG8=",
  "text": "hello"
}
```

Terminal event:

```json
{
  "exec_id": "exec_...",
  "seq": 3,
  "kind": "exit",
  "time_ns": 0,
  "status": "ok",
  "exit_code": 0
}
```

Error terminal event:

```json
{
  "exec_id": "exec_...",
  "seq": 3,
  "kind": "error",
  "time_ns": 0,
  "status": "error",
  "exit_code": -1,
  "error_code": "spawn_failed",
  "error_message": "..."
}
```

Rules:

1. `seq` is monotonically increasing per exec.
2. Exactly one terminal event is emitted.
3. `stdout` and `stderr` chunks preserve per-stream order.
4. Cross-stream ordering is best-effort based on read timing.
5. Binary-safe consumers use `data_base64`.
6. `text` is optional and present only for valid UTF-8 chunks.
7. The daemon may write the terminal event to a debug log, but the streamed terminal event is the
   P1 result.

## Session Signal

Endpoint:

```text
POST /v1/sessions/{session_id}/signal
```

Request:

```json
{
  "signal": "quiesce",
  "grace_timeout_ns": 10000000000
}
```

Supported signals:

- `quiesce`
- `resume`
- `terminate`
- `kill`

Semantics:

- `quiesce`: stop the VM, preserve runtime metadata and workspace.
- `resume`: restart a quiesced VM with the same image and workspace.
- `terminate`: graceful close, stop/remove the VM, preserve workspace unless cleanup policy
  says otherwise.
- `kill`: force close, stop/remove the VM, preserve workspace unless cleanup policy says
  otherwise.

P1 should implement `quiesce`, `resume`, and `terminate`. `kill` may map to forced terminate if the
runtime supports it.

## Filesystem API

All filesystem APIs operate on the session workspace root, not arbitrary VM paths.

Path rules:

1. Paths may be absolute with `/workspace` prefix or relative to the workspace.
2. Paths must be normalized and confined under `{state_root}/sessions/{session_id}/workspace`.
3. `..` escapes are rejected.
4. Existing symlinks that resolve outside the workspace are rejected.
5. Writes through symlinks are rejected unless the resolved target is inside the workspace.

Required endpoints:

- `GET /v1/sessions/{session_id}/fs/file?path=...`
- `PUT /v1/sessions/{session_id}/fs/file`
- `POST /v1/sessions/{session_id}/fs/edit`
- `POST /v1/sessions/{session_id}/fs/apply_patch`
- `POST /v1/sessions/{session_id}/fs/grep`
- `POST /v1/sessions/{session_id}/fs/glob`
- `GET /v1/sessions/{session_id}/fs/stat?path=...`
- `GET /v1/sessions/{session_id}/fs/exists?path=...`
- `GET /v1/sessions/{session_id}/fs/list_dir?path=...`

P1 should use direct host filesystem access under the workspace root. Do not shell out inside the VM
for file reads/writes.

Large file outputs should support `max_bytes` and `offset_bytes`. P1 does not need AOS CAS output
materialization; that belongs in the P3 adapter provider.

## Health And Inventory

Required endpoints:

- `GET /healthz`
- `GET /v1/host/info`
- `GET /v1/host/inventory`

`/v1/host/info` returns:

- host id,
- daemon version,
- runtime kind,
- runtime version if available,
- configured state root,
- configured resource defaults and max values,
- allowed images and network modes.

`/v1/host/inventory` returns all known sessions and their current runtime inspection status.

On startup the daemon should:

1. inspect smolvm machines with the fabric machine-name prefix for the current `host_id`,
2. classify running managed machines as `ready`,
3. classify stopped managed machines as `quiesced`,
4. scan `{state_root}/sessions/*/workspace` for directories without matching managed machines,
5. report unmatched directories as `orphaned_workspace`.

Because there is no daemon database, closed sessions do not need tombstones. A terminated session is
gone from active inventory once its managed machine is removed. Its workspace may remain as an
orphan if cleanup policy preserves it.

## Configuration

The host daemon should accept config from CLI flags and environment.

Required settings:

- `--listen`
- `--state-root`
- `--host-id`
- `--runtime` (`smolvm`)
- `--allowed-image`
- `--allowed-network-mode`
- `--allowed-host`
- `--allowed-cidr`
- `--default-network-mode`
- `--default-cpu-limit-millis`
- `--default-memory-limit-bytes`
- `--max-cpu-limit-millis`
- `--max-memory-limit-bytes`
- `--default-session-ttl-ns`
- `--max-session-ttl-ns`
- `--allow-ssh-agent`
- `--auth-token-file`
- `--allow-unauthenticated-loopback`

Defaults:

- listen on `127.0.0.1:7788`,
- state root `.aos/fabric-host`,
- runtime defaults to `smolvm`,
- unauthenticated loopback allowed for local development,
- non-loopback bind requires an auth token,
- default network mode `none`,
- `none` and `nat` are the only default allowed network modes,
- SSH agent forwarding disabled.

## Security Posture

P1 is a development substrate, but the defaults should not be reckless.

Required defaults:

- bind loopback only,
- run each session in a separate smolvm microVM,
- no host control socket mount,
- no arbitrary host mounts,
- only the per-session workspace is mounted read/write,
- resource limits are applied when configured,
- image allowlist is enforced,
- network mode allowlist is enforced,
- network disabled by default,
- SSH agent forwarding disabled by default,
- request size limit is enforced,
- filesystem paths are confined to the workspace,
- non-loopback listening requires bearer-token authentication.

Do not attempt a final multi-tenant sandbox story in P1. Document that the host daemon should run on
machines dedicated to fabric workloads until stronger isolation lands.

## Error Model

Use consistent JSON error bodies:

```json
{
  "status": "error",
  "error_code": "session_not_found",
  "error_message": "session 'sess_...' not found"
}
```

Recommended HTTP status mapping:

- `400`: invalid params,
- `401`: missing or invalid auth,
- `403`: policy or allowlist rejection,
- `404`: missing session/path,
- `409`: invalid state, duplicate session, or in-flight request key,
- `413`: request too large,
- `500`: daemon/runtime failure,
- `504`: timeout.

The `error_code` strings should be stable because the P3 adapter will translate them into
`Host*Receipt` error fields.

## Direct Smoke Flow

P1 should include a documented command sequence equivalent to:

```bash
cargo run -p aos-fabric --bin aos-fabric-host -- \
  --state-root .aos/fabric-host \
  --listen 127.0.0.1:7788 \
  --runtime smolvm \
  --allowed-image alpine:latest

curl -s http://127.0.0.1:7788/v1/sessions \
  -H 'content-type: application/json' \
  -d '{"image":"alpine:latest","network_mode":"none"}'

curl -N http://127.0.0.1:7788/v1/sessions/{session_id}/exec \
  -H 'content-type: application/json' \
  -H 'accept: application/x-ndjson' \
  -d '{"argv":["sh","-lc","printf hello"]}'
```

The smoke flow must also cover:

1. write a file,
2. read the file,
3. run a command that reads the file,
4. quiesce the session,
5. resume the session,
6. close the session.

## Tests

Unit tests:

- config parsing and defaulting,
- protocol JSON round trips,
- error envelope serialization,
- path confinement,
- symlink escape rejection,
- deterministic session id derivation from request id,
- runtime inventory classification,
- NDJSON event sequencing.

Integration tests:

- open session with allowed image,
- reject disallowed image,
- exec returns stdout/stderr and terminal event,
- concurrent exec calls both settle,
- timeout kills command and leaves session usable,
- write/read/list/stat/exists round trip,
- grep/glob round trip,
- edit/apply-patch round trip,
- quiesce/resume preserves workspace,
- daemon restart inventory finds managed smolvm machines,
- daemon restart inventory reports orphaned workspaces,
- terminate removes runtime machine.

Smolvm tests should be gated behind an e2e feature and an environment opt-in, for example:

```text
cargo test -p aos-fabric --features e2e-tests
AOS_FABRIC_E2E=1 cargo test -p aos-fabric --features e2e-tests
```

Tests must skip cleanly if smolvm is unavailable or the host lacks the required hypervisor/KVM
support.

## Implementation Order

1. Add crate, config, protocol types, and health endpoint.
2. Add stateless inventory classification from smolvm machines and workspace directories.
3. Add path confinement helpers and filesystem endpoints.
4. Add `SmolvmFacade`.
5. Implement `SmolvmFacade` using the Rust API directly.
6. Add session open and close.
7. Add exec streaming with NDJSON over the smolvm agent protocol.
8. Add quiesce/resume.
9. Add restart inventory reconciliation.
10. Add direct smoke docs and e2e tests.

## Definition Of Done

P1 is complete when:

1. `aos-fabric-host` can run locally without `aos-node`.
2. A direct client can open a smolvm session, run streaming exec, perform filesystem RPCs, and
   close the session.
3. Multiple exec calls can run concurrently against one session.
4. Quiesce/resume preserves the session workspace.
5. Restart inventory reports running/quiesced managed machines and orphaned workspaces correctly.
6. Smolvm tests are gated and skip cleanly when the runtime is unavailable.
7. The daemon has no dependency on AOS runtime crates and no daemon-owned database.
8. The P2 controller can call the P1 host API without changing the host protocol shape.
