# v0.20 Fabric

Fabric is the remote execution product layer for AgentOS host sessions.

The important boundary for v0.20 is:

1. AOS workflows keep emitting the existing `host.*` effects.
2. `aos-effect-adapters` gains a fabric-backed provider for those effects.
3. The fabric backend/controller is mostly standalone and owns containers, later VMs.

This avoids two competing terminal/session APIs. Fabric should become a backend for the host
effect surface, not a second parallel `fabric.*` tool surface.

## Current Runtime Assumptions

The v0.19 runtime refactors are assumed complete enough for this work:

- worlds record durable open external work before adapters start,
- opened async effects are published only after durable append,
- adapters are non-authoritative and return stream frames or terminal receipts,
- continuations re-enter only through owner admission,
- reads are hot in-process reads from active worlds,
- `aos-node` is the unified node library and `aos-cli` is the executable surface,
- adapter routing is backend-neutral through manifest `effect_bindings` plus host adapter routes.

Fabric must not weaken those rules. A fabric adapter may talk to a remote controller, but it still
only returns observed facts as `host.*` stream frames and receipts.

## Product Goal

The first fabric release should give an agent access to a Unix-like scratchpad session:

- open a session on a managed machine,
- run commands with stdout/stderr feedback,
- make multiple parallel RPC-style calls against the same session,
- keep sessions alive from seconds to days,
- quiesce or stop sessions,
- check out repositories and run normal Rust/Python/Node/Go toolchains,
- install missing tools inside the session,
- use semi-permanent per-session storage,
- optionally allow local-machine sessions when explicitly enabled.

This is for agent development and task execution, not permanent application deployment.

## Design Stance

### 1) Fabric is a host provider, not a new effect catalog

Keep these effect kinds as the AOS-facing API:

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

The fabric implementation registers provider adapter kinds such as:

- `host.session.open.fabric`
- `host.exec.fabric`
- `host.session.signal.fabric`
- `host.fs.read_file.fabric`

World manifests bind `host.*` effects to logical adapter IDs, and host config maps those adapter IDs
to fabric provider kinds. The existing local host adapters remain the local/dev provider.

### 2) Add a backend seam inside the host adapter implementation

The current local host adapter directly owns process execution and local session state. v0.20
should split that into:

1. shared host effect adapter wrappers,
2. a `HostBackend`-style execution trait,
3. `LocalHostBackend` for current local process/session behavior,
4. `FabricHostBackend` that proxies to the fabric controller.

The adapter wrappers keep:

- CBOR param decode and receipt encode,
- AOS Store/CAS output materialization,
- adapter route registration,
- stream frame emission,
- schema compatibility for `sys/Host*` types.

Backends own:

- session open/lookup/signal,
- exec start and output observation,
- filesystem operations,
- provider-specific session state.

### 3) Expand `HostTarget`, do not invent `FabricTarget` effects

The host target schema currently only supports `local`. v0.20 should extend it with a container
target that fabric can satisfy:

```text
HostTarget =
  local(HostLocalTarget)
  container(HostContainerTarget)
```

Suggested first `HostContainerTarget` fields:

- `image`
- `workdir`
- `env`
- `network_mode`
- `mounts`
- `cpu_limit_millis`
- `memory_limit_bytes`
- `ttl_ns`
- `labels`

The local backend can reject `container` with `unsupported_target`. The fabric backend can reject
`local` unless explicitly configured to proxy local-machine sessions.

### 4) Use simple HTTP plus NDJSON streaming first

Do not start with gRPC or WebSockets.

The first protocol should be:

- JSON request/response for session, filesystem, and control RPCs,
- HTTP chunked NDJSON for streaming exec output,
- one terminal event per exec stream,
- bearer token or mTLS between adapter, controller, and hosts.

NDJSON is simple to implement, simple to debug with `curl`, and maps directly to the current
adapter `ensure_started` API, which can emit `EffectStreamFrame` updates before the final receipt.

### 5) Keep fabric backend extractable

It is acceptable to start in this repository for faster integration. The backend should still be
written as if it may later move out:

- no dependency on `aos-node`,
- a small shared protocol/types crate if needed,
- controller and host daemons communicate through explicit HTTP APIs,
- AOS-specific code stays in the adapter provider.

## Architecture

```text
workflow
  emits host.* effect
    |
kernel/node
  records open work, flushes journal frame
    |
aos-effect-adapters
  host.* fabric provider
    |
fabric controller
  auth, idempotency, scheduling, session state
    |
fabric host daemon(s)
  container runtime, files, exec, logs, heartbeats
    |
OCI containers first, VMs later
```

### AOS Adapter Side

The fabric host provider lives in `aos-effect-adapters`.

Responsibilities:

- translate `HostSessionOpenParams` into controller session-open RPCs,
- pass `intent_hash` as the idempotency key,
- translate controller outputs into `Host*Receipt` payloads,
- store large stdout/stderr/file results in the AOS Store/CAS using current host output rules,
- emit stream frames for stdout/stderr chunks when the controller streams them,
- avoid authoritative session state beyond transient request state.

The adapter must not:

- mutate world state directly,
- start work before the node publishes opened effects after durable flush,
- make controller state authoritative for AOS replay,
- introduce a second host/fabric effect catalog for normal session operations.

### Fabric Controller

The controller is the centralized API and scheduler.

Responsibilities:

- authenticate AOS adapters and fabric hosts,
- maintain session records and idempotency records,
- choose a host for new sessions,
- track host capacity and liveness,
- proxy or redirect session RPCs to the assigned host,
- expose session status, logs, and terminal outcomes,
- reconcile controller state with host heartbeats after restart.

First state backend:

- SQLite.

This is enough for v0.20 and keeps the controller operationally small. Later, the state backend can
move to Postgres or a replicated store if fabric becomes multi-controller.

Controller records:

- hosts: `host_id`, endpoint, capabilities, resource capacity, last heartbeat, status,
- sessions: `session_id`, target, host_id, status, lease/ttl, labels, created/updated timestamps,
- execs: `exec_id`, session_id, idempotency key, status, exit code, timestamps, output refs,
- idempotency: external request key to created resource or terminal result.

### Fabric Host Daemon

The host daemon runs directly on a server. It is not a Kubernetes pod manager for sessions.

Responsibilities:

- register and heartbeat with the controller,
- manage local container runtime resources,
- create per-session containers and storage roots,
- run commands inside sessions,
- stream stdout/stderr to the controller/adapter,
- implement confined filesystem operations under the session root,
- enforce resource limits and network mode,
- report existing session inventory after restart.

First runtime driver:

- OCI containers through Docker or Podman.

Implementation can start with a CLI-backed driver for speed, but the driver should sit behind a
trait so it can move to Docker Engine, Podman API, containerd, Firecracker, or normal VMs later.

### State Authority

Fabric state is split intentionally:

- AOS remains authoritative for effect lifecycle, replay, and receipt admission.
- The fabric controller is authoritative for session allocation and idempotency.
- A fabric host is authoritative for the live container/process it owns.

The controller should recover by:

1. loading sessions from SQLite,
2. accepting host registration/heartbeat,
3. asking each host for live inventory,
4. reconciling missing sessions as `lost` or `closed`,
5. preserving idempotency records for already completed opens/execs.

## Protocol Sketch

The protocol is intentionally small. Define the host-daemon API first, then put a controller facade
in front of it. The adapter should only depend on the controller-facing API.

### Adapter to Controller

- `POST /v1/sessions`
  - opens a session,
  - request includes target, labels, ttl, resource hints, and AOS idempotency key,
  - response returns `session_id`, status, timestamps, assigned host metadata.

- `GET /v1/sessions/{session_id}`
  - returns current session status.

- `POST /v1/sessions/{session_id}/exec`
  - starts a command,
  - request includes argv, cwd, env patch, stdin bytes/ref, timeout, output mode,
  - response can be terminal JSON for non-streaming calls,
  - `Accept: application/x-ndjson` returns stdout/stderr/progress events and one terminal event.

- `POST /v1/sessions/{session_id}/signal`
  - stop, terminate, quiesce, or resume.

- `GET /v1/sessions/{session_id}/fs/file`
- `PUT /v1/sessions/{session_id}/fs/file`
- `POST /v1/sessions/{session_id}/fs/edit`
- `POST /v1/sessions/{session_id}/fs/apply_patch`
- `POST /v1/sessions/{session_id}/fs/grep`
- `POST /v1/sessions/{session_id}/fs/glob`
- `GET /v1/sessions/{session_id}/fs/stat`
- `GET /v1/sessions/{session_id}/fs/exists`
- `GET /v1/sessions/{session_id}/fs/list_dir`

### Controller to Host

The host API can mirror the controller API but should be host-authenticated and include controller
lease metadata. Hosts should reject requests for sessions they do not own.

Host registration:

- `POST /v1/hosts/register`
- `POST /v1/hosts/{host_id}/heartbeat`
- `GET /v1/hosts/{host_id}/inventory`

### NDJSON Exec Events

Minimum event kinds:

- `started`
- `stdout`
- `stderr`
- `exit`
- `error`

Each event includes:

- `exec_id`
- monotonically increasing `seq`
- optional bytes or text payload,
- timestamp,
- terminal status fields for `exit` or `error`.

The adapter converts these into AOS stream frames plus the final `HostExecReceipt`.

## Security And Policy

Kernel capability and policy checks remain first-line enforcement through `sys/host@1`.

Fabric adds operational enforcement:

- adapter-to-controller authentication,
- controller-to-host authentication,
- image allowlists,
- max CPU/memory/session TTL,
- network mode allowlists,
- no privileged containers by default,
- no host Docker socket inside sessions,
- no Docker-in-session for v0.20,
- path confinement for filesystem RPCs,
- per-session labels for tenant/world/session attribution,
- optional local-machine provider disabled by default.

## Implementation Plan

Implementation should start outside AOS and move inward:

1. prove one host can create and control container sessions,
2. add a controller that schedules and reconciles hosts,
3. add the AOS host-effect provider,
4. wire full end-to-end runtime tests.

### P1: Fabric Host Daemon With Container Sessions

- [ ] Add a standalone fabric crate or crate group with minimal AOS dependencies.
- [ ] Define the host-facing protocol request/response/event structs.
- [ ] Implement host daemon HTTP API for local development.
- [ ] Add container runtime trait.
- [ ] Implement a Docker/Podman-backed container runtime driver.
- [ ] Create per-session storage roots/volumes.
- [ ] Implement session open, exec, signal, and close.
- [ ] Implement filesystem RPCs against the confined session root.
- [ ] Stream exec stdout/stderr as NDJSON.
- [ ] Add a direct host-daemon smoke test or CLI path: open session, exec command, read/write file,
  close session.
- [ ] Add host daemon integration tests gated behind an e2e feature.

### P2: Fabric Controller Skeleton

- [ ] Implement controller HTTP API.
- [ ] Add SQLite state for hosts, sessions, execs, and idempotency.
- [ ] Implement host registration and heartbeat.
- [ ] Implement deterministic scheduler policy for v0.20: first healthy host with capacity.
- [ ] Proxy or dispatch session, exec, signal, and filesystem RPCs to the assigned host.
- [ ] Add controller-driven smoke test with one host daemon.
- [ ] Add controller unit tests for idempotency and host/session reconciliation.

### P3: AOS Host Adapter Provider

- [ ] Introduce a host backend trait used by all `host.*` adapters.
- [ ] Move current local behavior into `LocalHostBackend`.
- [ ] Keep existing tests passing against the local backend.
- [ ] Add `FabricHostBackend` that talks to the controller API.
- [ ] Add provider adapter kinds for fabric-backed `host.*` routes.
- [ ] Add fabric adapter config: controller URL, auth token path/env, request timeout.
- [ ] Extend `HostTarget` schemas/types with `container`.
- [ ] Add schema normalization tests for the new target variant.

### P4: AOS End-To-End Integration

- [ ] Wire fabric provider routes through `EffectAdapterConfig`.
- [ ] Add manifest examples binding `host.*` effects to fabric route IDs.
- [ ] Add e2e test: open container session, write file, exec command, read output, signal session.
- [ ] Add streaming test: long command emits stream frames before terminal receipt.
- [ ] Add replay test: admitted receipts replay without re-running fabric work.
- [ ] Add `aos` CLI dev command or documented command sequence for starting controller and one host.

## First Version Scope

In scope:

- one controller,
- one or more host daemons,
- container sessions,
- RPC-style exec,
- stdout/stderr streaming,
- session TTL and stop/quiesce,
- per-session semi-permanent storage,
- full `host.*` filesystem surface,
- local/dev config that can run controller and host on one machine.

Out of scope:

- Kubernetes-managed session pods,
- Firecracker or full VM sessions,
- SSH-backed external machines,
- permanent service deployment,
- shared volumes across sessions,
- cross-session networking,
- desktop or browser sessions,
- Docker inside sessions,
- long-running queued jobs with one-job-at-a-time scheduling.

## Later

- Firecracker VM runtime driver.
- Normal VM runtime driver.
- SSH session provider.
- Durable reusable volumes.
- Shared volumes and session networks.
- Browser and desktop session classes.
- Job queue semantics separate from immediate RPC exec.
- Multi-controller fabric state backend.
- Rich artifact/log browsing surfaces.
- Secret-provider integration for injecting scoped credentials into sessions.
