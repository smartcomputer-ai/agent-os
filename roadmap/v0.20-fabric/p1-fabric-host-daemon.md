# P1: Fabric Host Daemon With Smolvm Sessions

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (the AOS adapter and controller would otherwise be designed against an
unproven execution substrate)  
**Status**: In progress  
**Depends on**:
- `roadmap/v0.20-fabric/fabric.md`
- `roadmap/v0.20-fabric/rec.md`

## Goal

Build the first standalone fabric host daemon.

P1 should prove that one installed daemon on one Unix host can:

1. [x] create isolated smolvm-backed sessions from OCI images,
2. [x] run parallel RPC-style exec calls inside a session,
3. [x] stream stdout/stderr while commands run,
4. [x] expose confined filesystem operations against the session workspace,
5. [x] quiesce, resume, and close sessions,
6. [x] reconstruct live/quiesced sessions from smolvm runtime inventory after daemon restart,
7. [x] be exercised directly with `curl` or a small CLI before any controller or AOS adapter exists.

Current implementation note: the exec endpoint emits live NDJSON events from smolvm protocol frames,
and the CLI consumes those events without buffering the whole response body.

This phase was originally developed before Fabric moved into AOS. There is no
`aos-effect-adapters` work in P1 and no controller scheduling layer. P1 is the host substrate that
P2 and P3 build on.

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

Initial P1 used the compact standalone workspace scaffold. After the first P2 refactor, the current
crate shape is split so the controller can depend on protocol/client code without compiling smolvm:

```text
crates/fabric-protocol/
  Cargo.toml
  src/lib.rs

crates/fabric-client/
  Cargo.toml
  src/lib.rs

crates/fabric-host/
  Cargo.toml
  src/lib.rs
  src/main.rs
  src/config.rs
  src/http.rs
  src/state.rs
  src/service.rs
  src/fs.rs
  src/exec.rs
  src/runtime.rs
  src/smolvm.rs

crates/fabric-cli/
  Cargo.toml
  src/main.rs

third_party/
  smolvm/   # git submodule: https://github.com/smol-machines/smolvm.git
```

The Fabric crates should not depend on `aos-node`, `aos-kernel`, or `aos-effect-adapters`. Later
AOS integration should live in a separate crate, for example `fabric-aos-adapter`, and depend on
the fabric client/protocol surface.

Acceptable dependencies:

- async/runtime and HTTP crates,
- serialization crates,
- `smolvm` as the only runtime dependency,
- common utility crates.

Use the smolvm Rust API directly. P1 should not shell out to the `smolvm` CLI and should not call
the smolvm HTTP server as a sidecar.

### Smolvm Dependency Strategy

Use a pinned smolvm source dependency for P1.

Current initial shape:

```text
third_party/
  smolvm/   # git submodule: https://github.com/smol-machines/smolvm.git
```

Reference the crates directly from `crates/fabric-host/Cargo.toml`:

```toml
[dependencies]
smolvm = { path = "../../third_party/smolvm" }
smolvm-protocol = { path = "../../third_party/smolvm/crates/smolvm-protocol" }
```

Do not add `third_party/smolvm` as a workspace member. Treat it as an external source dependency.
This keeps the fabric workspace boundary clear while still allowing P1 to pin and patch smolvm
while the integration surface is settling.

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

Prefer keeping protocol and client types inside `crates/fabric` for P1. If the protocol stabilizes
and is shared by controller, CLI, and AOS adapter, P2 can split it into `fabric-protocol` and
`fabric-client`.

## Operating Model

P1 runs in direct-host mode:

```text
client or curl
  |
HTTP JSON / NDJSON
  |
fabric-host
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
  smolvm/
    smolvm.redb
  sessions/
    {session_id}/
      workspace/
      tmp/
      logs/
      fabric-session.json
```

Workspace directories are data, not daemon metadata. They can outlive machines, but a workspace
directory alone is not an active session. If a workspace exists without a managed runtime object,
inventory may report it as an orphaned workspace, not as a resumable session.

P2 will add the fabric controller and its durable state/idempotency tables. The host daemon should
not duplicate that database locally.

Required runtime identity:

- deterministic machine name: `fabric-{host_id_hash}-{session_id}`
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

Define a narrow runtime trait before wiring smolvm into the host service. This is not an open-ended
plugin system; it is just the boundary between fabric's HTTP/session semantics and smolvm's API
details. The current scaffold calls this trait `FabricRuntime`, with `SmolvmRuntime` as the only P1
implementation.

```rust
trait FabricRuntime {
    async fn open_session(&self, request: SessionOpenRequest) -> Result<SessionOpenResponse>;
    async fn session_status(&self, session_id: &SessionId) -> Result<SessionStatusResponse>;
    async fn exec_stream(&self, request: ExecRequest) -> Result<ExecEventStream>;
    async fn signal_session(
        &self,
        session_id: &SessionId,
        request: SignalSessionRequest,
    ) -> Result<SessionStatusResponse>;
}
```

P1 implementation:

- `SmolvmRuntime` implements `FabricRuntime` with the smolvm Rust API directly.
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

Actual smolvm API stance after reviewing the vendored code:

- Use `SmolvmDb::open_at(path)` for fabric's smolvm-owned machine registry.
- Use `VmRecord` and `RecordState` for persisted runtime records.
- Use `AgentManager::for_vm_with_sizes` plus `ensure_running_via_subprocess` for VM lifecycle.
- Use one fresh `AgentClient` per exec request so parallel exec RPCs do not serialize on a shared
  client.
- Use `AgentRequest::Run`, not `AgentClient::vm_exec`, for normal fabric commands. `vm_exec` runs
  in the agent VM rootfs, while fabric sessions must run in the requested OCI image rootfs.
- Use `RunConfig::with_persistent_overlay(Some(machine_name.clone()))` or the equivalent low-level
  `AgentRequest::Run { persistent_overlay_id: Some(machine_name), ... }` so package installs and
  image filesystem changes persist across exec calls in the same fabric session.

Known smolvm layout caveat:

- `SmolvmDb::open_at` lets fabric put smolvm's machine registry under `{state_root}/smolvm`.
- `AgentManager::for_vm_with_sizes` currently still places per-machine runtime artifacts under
  smolvm's platform cache root.
- P1 may tolerate this for the first proof, but the preferred follow-up is a small upstreamable
  smolvm API that lets embedded callers choose the machine runtime root.

Boot helper requirement:

Smolvm's safe server launch path currently calls `std::env::current_exe()` and spawns:

```text
<current-exe> _boot-vm <config-path>
```

When smolvm is linked into `fabric-host`, `current_exe()` is the fabric daemon binary, not the
`smolvm` CLI. P1 must handle this explicitly before using
`AgentManager::ensure_running_via_subprocess`.

Acceptable solutions:

1. Add a hidden `fabric-host _boot-vm <config-path>` command that forwards to public smolvm
   boot logic.
2. Upstream a small smolvm API change so callers can configure the boot helper executable or call a
   public `boot_vm(config_path)` function.

Prefer the upstreamable smolvm API change if it is small. Do not work around this by invoking the
smolvm CLI for normal lifecycle operations.

Session creation flow:

1. Derive `machine_name = fabric-{host_id_hash}-{session_id}`.
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
8. Pull the session image through `manager.connect()?.pull_with_registry_config`.
9. Return `ready` only after VM startup and image pull both succeed.

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

1. send `AgentRequest::Run { interactive: true, tty: false, persistent_overlay_id:
   Some(machine_name), ... }` with `AgentClient::send_raw`,
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
  - `fabric-{host_id_hash}-{session_id}`
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
  "kind": "stdout"
}
```

Output event:

```json
{
  "exec_id": "exec_...",
  "seq": 2,
  "kind": "stdout",
  "text": "hello"
}
```

Terminal event:

```json
{
  "exec_id": "exec_...",
  "seq": 3,
  "kind": "exit",
  "exit_code": 0
}
```

Error terminal event:

```json
{
  "exec_id": "exec_...",
  "seq": 3,
  "kind": "error",
  "message": "..."
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

- [x] `GET /v1/sessions/{session_id}/fs/file?path=...`
- [x] `PUT /v1/sessions/{session_id}/fs/file`
- [x] `POST /v1/sessions/{session_id}/fs/edit`
- [x] `POST /v1/sessions/{session_id}/fs/apply_patch`
- [x] `POST /v1/sessions/{session_id}/fs/grep`
- [x] `POST /v1/sessions/{session_id}/fs/glob`
- [x] `GET /v1/sessions/{session_id}/fs/stat?path=...`
- [x] `GET /v1/sessions/{session_id}/fs/exists?path=...`
- [x] `GET /v1/sessions/{session_id}/fs/list_dir?path=...`

P1 should use direct host filesystem access under the workspace root. Do not shell out inside the VM
for file reads/writes.

Large file outputs should support `max_bytes` and `offset_bytes`. P1 does not need AOS CAS output
materialization; that belongs in the P3 adapter provider.

## Health And Inventory

Required endpoints:

- [x] `GET /healthz`
- [x] `GET /v1/host/info`
- [x] `GET /v1/host/inventory`

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
- state root `.fabric-host`,
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
  "code": "not_found",
  "message": "not found: session 'sess_...'"
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

The `code` strings should be stable because the P3 adapter will translate them into
`Host*Receipt` error fields.

Current implementation note: the wire shape is `code` and `message`. The `code` field is semantic
and stable at the host boundary (`invalid_request`, `conflict`, `not_found`, `not_implemented`,
`runtime_error`).

## Direct Smoke Flow

P1 should include a documented command sequence equivalent to:

```bash
dev/fabric/bootstrap-smolvm-release.sh
dev/fabric/build-fabric-host.sh

LIBKRUN_BUNDLE="$PWD/third_party/smolvm-release/lib" \
DYLD_LIBRARY_PATH="$PWD/third_party/smolvm-release/lib" \
SMOLVM_AGENT_ROOTFS="$PWD/third_party/smolvm-release/agent-rootfs" \
target/debug/fabric-host \
  --state-root .fabric-host \
  --bind 127.0.0.1:8791 \
  --host-id local-dev

curl -s http://127.0.0.1:8791/v1/sessions \
  -H 'content-type: application/json' \
  -d '{"session_id":"sess-smoke","image":"alpine:latest","network_mode":"egress"}'

curl -N http://127.0.0.1:8791/v1/sessions/sess-smoke/exec \
  -H 'content-type: application/json' \
  -H 'accept: application/x-ndjson' \
  -d '{"session_id":"sess-smoke","argv":["sh","-lc","printf hello"]}'
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

- [ ] config parsing and defaulting,
- [ ] protocol JSON round trips,
- [ ] error envelope serialization,
- [x] path confinement,
- [x] symlink escape rejection,
- [ ] deterministic session id derivation from request id,
- [x] runtime inventory classification,
- [ ] NDJSON event sequencing,
- [x] patch parser and apply/edit matching behavior.

Integration tests:

- [x] open session with allowed image,
- [ ] reject disallowed image,
- [x] exec returns stdout/stderr and terminal event,
- [x] concurrent exec calls both settle,
- [ ] timeout kills command and leaves session usable,
- [ ] write/read/list/stat/exists round trip,
- [ ] grep/glob round trip,
- [ ] edit/apply-patch round trip,
- [x] quiesce/resume preserves workspace,
- [ ] daemon restart inventory finds managed smolvm machines,
- [ ] daemon restart inventory reports orphaned workspaces,
- [ ] terminate removes runtime machine.

Manual smokes completed locally:

- [x] open smolvm session,
- [x] write/read workspace files,
- [x] exec command reads `/workspace`,
- [x] grep/glob workspace files,
- [x] edit/apply-patch workspace files,
- [x] quiesce/resume preserves workspace,
- [x] restart inventory reconstructs quiesced smolvm sessions,
- [x] inventory reports orphaned workspaces,
- [x] close session.

Smolvm tests are gated behind an environment opt-in and a wrapper script:

```text
dev/fabric/test-smolvm-e2e.sh
FABRIC_SMOLVM_E2E=1 cargo test -p fabric-host --test smolvm_e2e
```

Tests must skip cleanly if smolvm is unavailable or the host lacks the required hypervisor/KVM
support.

## Implementation Order

1. [x] Add crate scaffold, config, protocol types, client, and health endpoint.
2. [x] Add smolvm path dependencies and hidden `fabric-host _boot-vm <config-path>`.
3. [x] Implement `SmolvmRuntime::open` with `SmolvmDb::open_at(...).init_tables()`.
4. [x] Add session directory and marker-file creation.
5. [x] Add `POST /v1/sessions` backed by `AgentManager::ensure_running_via_subprocess` and image pull.
6. [x] Add live NDJSON exec streaming over low-level `AgentRequest::Run` / `AgentResponse` frames.
7. [x] Add path confinement helpers and direct host workspace filesystem endpoints.
8. [x] Add close, quiesce, and resume.
9. [x] Add restart inventory reconciliation from smolvm records and workspace directories.
10. [x] Add direct smoke docs and gated smolvm e2e harness.

## Definition Of Done

P1 is complete when:

1. [x] `fabric-host` can run locally without `aos-node`.
2. [x] A direct client can open a smolvm session, run streaming exec, perform filesystem RPCs, and
   close the session.
3. [x] Multiple exec calls can run concurrently against one session.
4. [x] Quiesce/resume preserves the session workspace.
5. [x] Restart inventory reports running/quiesced managed machines and orphaned workspaces correctly.
6. [x] Smolvm tests are gated and skip cleanly when the runtime is unavailable.
7. [x] The daemon has no dependency on AOS runtime crates and no daemon-owned database.
8. [ ] The P2 controller can call the P1 host API without changing the host protocol shape.
   This remains a P2 validation item; the P1 host API shape is stable enough to start controller work.
