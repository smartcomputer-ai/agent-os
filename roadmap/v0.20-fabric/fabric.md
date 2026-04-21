# v0.20 Fabric

Fabric is the remote execution product layer for AgentOS host sessions. Product-wise, Fabric
brokers scoped access to computers. Some computers are ephemeral sandboxes that Fabric creates and
owns, such as smolvm-backed OCI-image VMs. Others are existing machines that have a Fabric daemon
installed on them, such as bare-metal servers, externally deployed VMs, or pods.

The important boundary for v0.20 is:

1. AOS workflows keep emitting the existing `host.*` effects.
2. `aos-effect-adapters` gains a fabric-backed provider for those effects.
3. The fabric backend/controller is mostly standalone and owns session allocation, regardless of
   whether the backing computer is a Fabric-managed sandbox or an attached existing host.

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

The first operational substrate is Fabric-managed smolvm sessions, but the controller API should be
designed around a broader model: a session is a scoped lease on a computer. For managed sandboxes,
opening a session may create a new VM. For attached hosts, opening a session creates an access
lease/workspace on an already-existing machine and must not imply ownership of the machine
lifecycle.

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

### 3) Model targets and providers as tagged sum types

The host target schema currently only supports `local`. v0.20 should extend it with explicit target
variants that Fabric can satisfy. Do not invent a second `fabric.*` effect catalog; keep the
AOS-facing effects as `host.*`, but make the target object precise.

```text
HostTarget =
  local(HostLocalTarget)
  sandbox(HostSandboxTarget)
  attached_host(HostAttachedTarget)
```

Use tagged sum types with variant-specific records for all variant objects. Do not model target,
provider, selector, or signal variants as optional-field bags. The preferred wire shape is
`kind` plus `spec`, for example:

```json
{
  "target": {
    "kind": "sandbox",
    "spec": {
      "image": "docker.io/library/alpine:latest",
      "runtime_class": "smolvm",
      "network_mode": "egress"
    }
  }
}
```

Suggested first `HostSandboxTarget` fields:

- `image`
- `runtime_class`
- `workdir`
- `env`
- `network_mode`
- `mounts`
- `cpu_limit_millis`
- `memory_limit_bytes`
- `ttl_ns`
- `labels`

Suggested first `HostAttachedTarget` fields:

- `selector`
- `workdir`
- `workspace_policy`
- `user`
- `env`
- `ttl_ns`
- `labels`

`selector` should also be a tagged sum, such as `host_id(HostId)`, `pool(String)`, or
`labels(map)`. A controller may accept the attached-host target shape before the attached-host
daemon is implemented, but it should reject unsupported targets with a stable `unsupported_target`
error rather than degrading into ad hoc optional fields.

The local backend can reject `sandbox` and `attached_host` with `unsupported_target`. The fabric
backend can reject `local` unless explicitly configured to proxy local-machine sessions.

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
  provider capabilities, files, exec, logs, heartbeats
    |
managed smolvm microVMs or attached existing machines
```

### AOS Adapter Side

The fabric host provider lives in `aos-effect-adapters`.

Responsibilities:

- translate `HostSessionOpenParams` into controller session-open RPCs,
- pass `intent_hash` as the idempotency key,
- translate controller outputs into `Host*Receipt` payloads,
- store large stdout/stderr/file results in the AOS Store/CAS using current host output rules,
- emit coalesced time-based exec progress frames for long-running commands rather than one AOS
  frame per controller stdout/stderr event,
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
- choose a host/provider for new sessions,
- track host capacity and liveness,
- proxy or redirect session RPCs to the assigned host,
- expose session status, logs, and terminal outcomes,
- reconcile controller state with host heartbeats after restart.

First state backend:

- SQLite.

This is enough for v0.20 and keeps the controller operationally small. Later, the state backend can
move to Postgres or a replicated store if fabric becomes multi-controller.

Controller records:

- hosts: `host_id`, endpoint, provider capabilities, resource capacity, last heartbeat, status,
- sessions: `session_id`, target, host_id, status, lease/ttl, labels, created/updated timestamps,
- execs: `exec_id`, session_id, idempotency key, status, exit code, timestamps, output refs,
- idempotency: external request key to created resource or terminal result.

### Fabric Host Daemon

The host daemon runs on a computer that Fabric can use. In P1 it manages local smolvm runtime
resources and creates one VM per session. Later, the same daemon should also support an
attached-host provider mode where it exposes the existing machine itself through scoped sessions.

The host daemon is not a Kubernetes pod manager for sessions. It may run inside an externally
created pod or VM, but in that case the pod/VM is attached infrastructure from Fabric's point of
view unless Fabric also owns the provisioning layer.

Responsibilities:

- register and heartbeat with the controller,
- advertise provider capabilities as tagged sum types,
- manage local smolvm runtime resources when running the smolvm provider,
- create per-session microVMs and storage roots for sandbox sessions,
- create per-session leases/workspaces for attached-host sessions,
- run commands inside sessions,
- stream stdout/stderr to the controller/adapter,
- implement confined filesystem operations under the session root,
- enforce resource limits and network mode,
- report existing session inventory after restart.

Runtime driver:

- smolvm through the Rust API directly.

Do not implement a CLI-backed smolvm path, smolvm HTTP sidecar path, Docker/Podman path,
Firecracker path, QEMU path, or normal-VM fallback backend in P1.

The attached-host daemon is tracked separately as a later tentative phase. See
`roadmap/v0.20-fabric/p10-attached-host-daemon.md`.

### State Authority

Fabric state is split intentionally:

- AOS remains authoritative for effect lifecycle, replay, and receipt admission.
- The fabric controller is authoritative for session allocation and idempotency.
- A fabric host is authoritative for the live VM/process it owns.

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
  - request includes a tagged target sum, labels, ttl, resource hints, and AOS idempotency key,
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

Registration and heartbeat payloads should advertise provider capabilities as tagged sum types, for
example `smolvm(SmolvmProviderInfo)` and `attached_host(AttachedHostProviderInfo)`. The scheduler
uses the requested target kind plus provider capabilities to choose a host.

### NDJSON Exec Events

Minimum event kinds:

- `started`
- `stdout`
- `stderr`
- `exit`
- `error`

Each controller NDJSON event includes:

- `exec_id`
- monotonically increasing `seq`
- optional bytes or text payload,
- timestamp,
- terminal status fields for `exit` or `error`.

The Fabric AOS adapter consumes these events continuously, but it does not expose every controller
event as an AOS stream frame. It aggregates observed output and emits time-based `host.exec`
progress frames, for example every 10 seconds. If the exec finishes before the first interval, the
adapter emits no progress frames. In all cases, the terminal `HostExecReceipt` contains the complete
exec result.

## Security And Policy

Kernel capability and policy checks remain first-line enforcement through `sys/host@1`.

Fabric adds operational enforcement:

- adapter-to-controller authentication,
- controller-to-host authentication,
- image allowlists,
- max CPU/memory/session TTL,
- network mode allowlists,
- one smolvm microVM per session by default,
- attached-host sessions cannot terminate or power-manage the underlying machine,
- no host control socket inside sessions,
- no Docker-in-session for v0.20,
- path confinement for filesystem RPCs,
- per-session labels for tenant/world/session attribution,
- optional local-machine provider disabled by default.

## Implementation Plan

Implementation started by proving standalone Fabric components, then moved the generic Fabric
runtime into this AOS workspace:

1. prove one host can create and control smolvm sessions,
2. add a controller that schedules and reconciles hosts,
3. add the AOS host-effect provider,
4. wire full end-to-end runtime tests.

### P1: Fabric Host Daemon With Smolvm Sessions

Detailed spec: `roadmap/v0.20-fabric/p1-fabric-host-daemon.md`.

Status: complete for the v0.20 first cut. The host daemon substrate exists, has been manually
smoked with real smolvm sessions, and has been validated through the controller and AOS Fabric
adapter live e2e path.

- [x] Add a standalone fabric crate or crate group with minimal AOS dependencies.
- [x] Define the host-facing protocol request/response/event structs.
- [x] Implement host daemon HTTP API for local development.
- [x] Add a narrow smolvm facade used by the host service.
- [x] Implement smolvm integration using the Rust API directly.
- [x] Create per-session storage roots/volumes.
- [x] Implement session open, exec, signal, and close.
- [x] Implement filesystem RPCs against the confined session root.
- [x] Stream exec stdout/stderr as NDJSON.
- [x] Add a direct host-daemon smoke test or CLI path: open session, exec command, read/write file,
  close session.
- [x] Add host daemon integration tests gated behind an e2e feature.

### P2: Fabric Controller Skeleton

Detailed spec: `roadmap/v0.20-fabric/p2-fabric-controller.md`.

- Status: first cut complete for the unauthenticated controller path; bearer auth is deferred.

- [x] Implement controller HTTP API.
- [x] Model controller targets, host providers, selectors, and supported signal kinds as tagged
  sum types or explicit enums with variant-specific records where needed.
- [x] Add SQLite state for hosts, sessions, execs, and idempotency.
- [x] Implement host registration and heartbeat.
- [x] Implement deterministic scheduler policy for v0.20: first healthy host with capacity.
- [x] Proxy or dispatch session, exec, signal, and filesystem RPCs to the assigned host.
- [x] Add controller-driven smoke test with one host daemon.
- [x] Add controller unit tests for idempotency and host/session reconciliation.
- [x] Add OpenAPI discovery for controller and host APIs.

### P3: AOS Host Adapter Provider

Detailed spec: `roadmap/v0.20-fabric/p3-aos-host-adapter.md`.

- Status: complete for the v0.20 first cut.

- [x] Introduce a host backend boundary used by `host.*` adapters.
- [x] Move current local behavior behind the local host module boundary.
- [x] Aggressively refactor AOS host/runtime plumbing where needed; existing tests may be updated
  around the new boundary.
- [x] Extend Fabric protocol/client for binary exec stdin and binary file read/write.
- [x] Add Fabric-client exec aggregation/progress utilities for adapter use.
- [x] Add `FabricHostBackend` that talks to the controller API.
- [x] Add provider adapter kinds for fabric-backed `host.*` routes.
- [x] Add fabric adapter config: controller URL, auth token path/env, request timeout, exec
  progress interval.
- [x] Extend `HostTarget` schemas/types with `sandbox`.
- [x] Defer final host capability enforcement for sandbox targets to a later policy phase.
- [x] Add schema normalization tests for the new target variant.
- [x] Thread effect origin metadata into adapter startup so Fabric exec progress frames are
  admitted.

### P4: AOS End-To-End Integration

- Status: partially complete. The adapter-level and live Fabric e2e coverage exists; manifest
  examples, replay coverage, and a single controller-plus-host startup command sequence remain.
  AOS-side migration validation has passed, including the Fabric Cargo gates, smolvm host build,
  host smolvm e2e, and controller-plus-smolvm e2e checks from this checkout.

- [x] Wire fabric provider routes through `EffectAdapterConfig`.
- [ ] Add manifest examples binding `host.*` effects to fabric route IDs.
- [x] Add e2e test: open smolvm session, write file, exec command, read output, signal session.
- [x] Add streaming test: long command emits time-based progress frames before terminal receipt.
- [ ] Add replay test: admitted receipts replay without re-running fabric work.
- [ ] Add `aos` CLI dev command or documented command sequence for starting controller and one host.

### P10: Attached Host Daemon (Tentative)

Detailed spec: `roadmap/v0.20-fabric/p10-attached-host-daemon.md`.

- [ ] Add an attached-host provider mode to `fabric-host`.
- [ ] Register existing machines with the same controller and scheduler as smolvm hosts.
- [ ] Open sessions as scoped leases/workspaces on an existing machine, not as machine creation.
- [ ] Implement exec, filesystem, and close semantics against the attached workspace.
- [ ] Reject unsupported machine lifecycle operations such as runtime termination with stable
  capability errors.
- [ ] Add controller smoke tests using one attached host without requiring smolvm.

## First Version Scope

In scope:

- one controller,
- one or more host daemons,
- tagged target/provider API shape that can distinguish managed sandboxes from attached hosts,
- smolvm-backed OCI-image sessions,
- RPC-style exec,
- stdout/stderr streaming,
- session TTL and stop/quiesce,
- per-session semi-permanent storage,
- full `host.*` filesystem surface,
- local/dev config that can run controller and host on one machine.

Out of scope:

- Kubernetes-managed session pods,
- production attached-host daemon mode,
- non-smolvm managed runtime drivers,
- SSH-backed external machines,
- permanent service deployment,
- shared volumes across sessions,
- cross-session networking,
- desktop or browser sessions,
- Docker inside sessions,
- long-running queued jobs with one-job-at-a-time scheduling.

## Later

- Attached host daemon mode for bare-metal servers, externally deployed VMs, and externally
  deployed pods.
- SSH session provider.
- Durable reusable volumes.
- Shared volumes and session networks.
- Browser and desktop session classes.
- Job queue semantics separate from immediate RPC exec.
- Multi-controller fabric state backend.
- Rich artifact/log browsing surfaces.
- Secret-provider integration for injecting scoped credentials into sessions.
