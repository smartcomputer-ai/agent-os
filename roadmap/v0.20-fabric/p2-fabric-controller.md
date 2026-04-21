# P2: Fabric Controller Skeleton

**Priority**: P2  
**Effort**: High  
**Risk if deferred**: High (the AOS adapter would otherwise bind directly to a host daemon shape
instead of the durable scheduling and idempotency boundary)
**Status**: First cut complete for the unauthenticated controller path; bearer auth is deferred.
**Depends on**:
- `roadmap/v0.20-fabric/fabric.md`
- `roadmap/v0.20-fabric/p1-fabric-host-daemon.md`
- `roadmap/v0.20-fabric/rec.md`

## Goal

Build the first standalone Fabric controller.

P2 should prove that one controller can:

1. register one or more Fabric host daemons,
2. track host liveness and provider capabilities,
3. accept controller-facing session requests with tagged target records,
4. schedule sandbox sessions onto healthy smolvm-capable hosts,
5. persist hosts, sessions, idempotent execs, and idempotency records in SQLite,
6. proxy exec, signal, and filesystem RPCs to the assigned host,
7. stream exec stdout/stderr through the controller as NDJSON,
8. reconcile controller state with host inventory after restart,
9. be exercised directly with `curl` or `fabric-cli` before any AOS adapter exists.

P2 remains independent from AOS runtime crates. There is no `aos-effect-adapters` work in P2. The
controller API should be the API that the P3 Fabric adapter calls.

## Completed Surface

The first P2 controller cut now has:

- [x] split protocol/client/host/controller crate shape with smolvm isolated to `fabric-host`,
- [x] split `fabric-client` into explicit `FabricControllerClient` and `FabricHostClient`
  modules with shared HTTP/NDJSON decoding utilities,
- [x] split `fabric-cli` into explicit `fabric host`/`fabric h` and
  `fabric controller`/`fabric c` command groups,
- [x] standalone `fabric-controller` binary and testable controller library modules,
- [x] dynamic host registration and heartbeat from `fabric-host`,
- [x] SQLite-backed hosts, host providers, host labels, host inventory, controller sessions,
  session labels, idempotency records, exec records, and exec events,
- [x] typed controller session targets/providers/signals using tagged sum types,
- [x] deterministic sandbox scheduling across healthy smolvm-capable hosts,
- [x] controller session open/status/list and label patching,
- [x] idempotent controller session open replay and conflict detection,
- [x] controller-mediated exec proxying with NDJSON streaming, event persistence, replay, and
  conflict detection,
- [x] controller-mediated filesystem proxy routes,
- [x] controller-mediated signal proxy routes with lifecycle capability checks,
- [x] host inventory reconciliation after heartbeat and controller restart,
- [x] fake-host controller integration coverage,
- [x] gated real-smolvm controller E2E coverage via `dev/fabric/test-controller-smolvm-e2e.sh`.
- [x] OpenAPI discovery for controller and host APIs at `/docs` and `/openapi.json`.

Still intentionally open:

- [ ] bearer-token auth for controller clients and host registration/heartbeat,
- [ ] AOS `FabricHostBackend` integration, which starts in P3.

## Why This Exists

P1 proved the execution substrate. P2 adds the control-plane boundary that AOS should depend on.

The controller owns:

1. host registration and liveness,
2. target-to-host scheduling,
3. durable session records,
4. durable idempotency,
5. recovery and reconciliation after process restart,
6. a stable API that can later support attached hosts without changing the AOS-facing effect
   surface.

The AOS adapter must not talk directly to arbitrary host daemons. A direct adapter-to-host design
would push scheduling, idempotency, and replay recovery into the adapter layer, which is the wrong
authority boundary.

## Non-Goals

P2 does not implement:

- AOS effect adapter routes,
- `HostBackend`, `LocalHostBackend`, or `FabricHostBackend`,
- AOS Store/CAS output materialization,
- durable AOS replay tests,
- attached-host execution semantics,
- SSH-backed hosts,
- Kubernetes scheduling,
- non-smolvm managed runtime drivers,
- multi-controller state replication,
- Postgres state backend,
- queued job semantics,
- rich log or artifact browsing UI.

P2 should model attached-host targets and provider records as tagged sums, but it does not need to
implement a real attached-host daemon. That is tracked in
`roadmap/v0.20-fabric/p10-attached-host-daemon.md`.

## Product Model

Fabric brokers scoped access to computers.

For P2, the only fully implemented provider is the smolvm sandbox provider from P1:

```text
controller session open
  target = sandbox(...)
    |
controller schedules healthy smolvm host
    |
host daemon creates or resumes smolvm-backed session
```

The controller protocol should still make room for attached hosts:

```text
controller session open
  target = attached_host(...)
    |
controller schedules matching attached-host provider
    |
host daemon creates lease/workspace on existing machine
```

The second flow may return `unsupported_target` until P10 exists, but the type shape should already
be explicit.

## Request Routing Model

P2 uses controller-mediated routing. Clients do not connect directly to Fabric hosts or VMs for
normal session operations.

```text
client or AOS adapter
  |
  | controller-facing HTTP API
  v
fabric-controller
  |
  | host-facing HTTP API
  v
fabric-host
  |
  | provider/runtime API
  v
smolvm session or attached host workspace
```

For P3, the AOS `FabricHostBackend` should only know the controller URL and controller auth
credentials. It should not need host endpoints, smolvm machine names, or provider-local topology.

The controller is responsible for:

- authenticating clients and hosts,
- maintaining idempotency records,
- choosing a host/provider for new sessions,
- storing session-to-host assignment,
- looking up the assigned host for session-scoped operations,
- proxying JSON filesystem/status/signal RPCs to the host,
- streaming NDJSON exec events from the host back to the client,
- persisting terminal exec state and replayable idempotent results.

The host is responsible for:

- advertising its endpoint and provider capabilities,
- maintaining live local runtime/session state,
- enforcing workspace filesystem confinement,
- running commands and streaming output,
- reporting inventory to the controller.

Exec path:

```text
POST /v1/sessions/{session_id}/exec
  client -> controller
  controller loads session.host_id and host.endpoint
  controller -> host POST /v1/sessions/{session_id}/exec
  host -> controller NDJSON stdout/stderr/terminal events
  controller persists and forwards events to client
```

Direct client-to-host access may be useful for privileged debugging, but it is not part of the
normal Fabric API contract for P2. Starting with controller-mediated routing keeps auth, replay,
scheduling, and liveness policy in one place.

## Host Discovery Model

Dynamic host registration is the required P2 discovery path.

On startup, `fabric-host` calls:

```text
POST /v1/hosts/register
```

with:

- `host_id`,
- advertised controller-reachable endpoint,
- tagged provider capability records,
- labels such as pool, region, owner, or environment.

Then the host periodically calls:

```text
POST /v1/hosts/{host_id}/heartbeat
```

with refreshed provider capacity and, when available, an inventory snapshot.

The controller stores host records in SQLite and schedules only hosts whose heartbeat is current.
Static seed hosts may be supported as a local-development convenience, but they are not the primary
discovery model and should not replace registration/heartbeat.

## Crate Shape

Extend the current workspace rather than creating a separate repository, but do not keep growing one
monolithic `fabric` crate.

The controller should not compile or link smolvm. The current `fabric` crate already avoids smolvm
when the `smolvm-runtime` feature is disabled, and `fabric-controller` could technically depend on
`fabric` without that feature. That is acceptable for a short transition, but it is not the desired
P2 shape because Cargo feature unification can still enable smolvm when building multiple workspace
members together, and controller dependencies such as SQLite should not become host/CLI
dependencies.

Preferred P2 crate shape:

```text
crates/fabric-protocol/
  Cargo.toml
  src/lib.rs
  # IDs, errors, request/response/event records, tagged target/provider/signal sums

crates/fabric-client/
  Cargo.toml
  src/lib.rs
  # reqwest client, auth headers, NDJSON stream decoder

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
  # host executable plus testable internals; this package owns smolvm/libkrun

crates/fabric-controller/
  Cargo.toml
  src/lib.rs
  src/main.rs
  src/auth.rs
  src/config.rs
  src/http.rs
  src/proxy.rs
  src/reconcile.rs
  src/scheduler.rs
  src/service.rs
  src/state.rs
  # controller executable plus testable internals; no smolvm dependency

crates/fabric-cli/
  Cargo.toml
  src/main.rs
```

Dependency direction:

```text
fabric-protocol
  <- fabric-client
  <- fabric-host
  <- fabric-controller
  <- fabric-cli
```

The executable names should be `fabric-host` and `fabric-controller`; avoid the `hostd` or
`controllerd` suffix in the product-facing names.

`fabric-controller` may depend on `fabric-client` to call host daemons. It must not depend on
`fabric-host`.

`fabric-host` is the host package and executable. Put testable host logic in `src/lib.rs` and keep
process wiring in `src/main.rs`. It owns concrete process and provider concerns: CLI/env parsing,
tracing setup, smolvm boot-helper dispatch, smolvm provider implementation, controller
registration/heartbeat tasks, signal handling, and serving the router.

This keeps smolvm out of libraries that the controller needs while avoiding an extra provider crate
before there is a real second provider. Later, if the smolvm provider needs reuse outside
`fabric-host`, it can move into a `fabric-smolvm` crate without changing the controller API.

`fabric-controller` follows the same package pattern. Put reusable controller logic in `src/lib.rs`
and modules inside the `fabric-controller` package so integration tests can import it. Split out a
separate controller library only if another crate needs to embed the controller service.

Keep all Fabric crates independent from `aos-node`, `aos-kernel`, and `aos-effect-adapters`.

### Migration Stance

The existing `crates/fabric` crate currently contains protocol, client, host service, filesystem,
and smolvm integration behind a feature flag. Before or at the start of P2, split it along the
boundaries above.

Minimum acceptable split if time is tight:

1. move smolvm integration into `fabric-host`,
2. put controller code in `fabric-controller`,
3. keep protocol/client/host code in the existing `fabric` crate temporarily,
4. ensure `fabric-controller` depends on `fabric` without smolvm features and does not depend on
   `fabric-host`.

The preferred split is `fabric-protocol`, `fabric-client`, `fabric-host`, and `fabric-controller`.
This makes it impossible for the controller to pick up libkrun or smolvm through feature unification
while keeping the number of crates modest.

### Dependency Placement

Controller-only dependencies:

- SQLite client crate,
- migration helper if the SQLite client does not provide one,
- request middleware/auth helpers,
- common utility crates.

Suggested SQLite dependency:

```toml
sqlx = { version = "0.8", default-features = false, features = [
  "runtime-tokio-rustls",
  "sqlite",
  "migrate",
  "json",
] }
```

Avoid `sqlx::query!` compile-time macros in P2 unless the repo also commits the required offline
metadata. Runtime-checked queries are acceptable for the controller skeleton.

Smolvm and libkrun dependencies must remain isolated to `fabric-host`.

## Protocol Design

### Typed Variant Rule

All variant-bearing protocol objects must be tagged sum types with variant-specific records. Do not
model variants as optional-field bags.

Use `kind` plus `spec` on the wire:

```json
{
  "kind": "sandbox",
  "spec": {
    "image": "docker.io/library/alpine:latest",
    "runtime_class": "smolvm"
  }
}
```

Use this pattern for:

- session targets,
- host providers,
- host selectors,
- lifecycle signals,
- capability records if they become variant-specific.

### Common Identifiers

Keep the existing transparent IDs and add small wrappers where useful:

```rust
pub struct SessionId(pub String);
pub struct ExecId(pub String);
pub struct HostId(pub String);
pub struct RequestId(pub String);
```

`request_id` is the external idempotency key. The AOS adapter will pass the effect `intent_hash`
as this value in P3. Direct users may omit it for non-idempotent development calls.

### Controller Session Open

Controller-facing open requests should be separate from P1 host-facing `SessionOpenRequest`.

```rust
pub struct ControllerSessionOpenRequest {
    pub request_id: Option<RequestId>,
    pub target: FabricSessionTarget,
    pub ttl_ns: Option<u128>,
    pub labels: BTreeMap<String, String>,
}

#[serde(tag = "kind", content = "spec", rename_all = "snake_case")]
pub enum FabricSessionTarget {
    Sandbox(FabricSandboxTarget),
    AttachedHost(FabricAttachedHostTarget),
}

pub struct FabricSandboxTarget {
    pub image: String,
    pub runtime_class: Option<String>,
    pub workdir: Option<String>,
    pub env: BTreeMap<String, String>,
    pub network_mode: NetworkMode,
    pub mounts: Vec<MountSpec>,
    pub resources: ResourceLimits,
}

pub struct FabricAttachedHostTarget {
    pub selector: HostSelector,
    pub workspace_policy: AttachedWorkspacePolicy,
    pub workdir: Option<String>,
    pub user: Option<String>,
    pub env: BTreeMap<String, String>,
}
```

Example sandbox request:

```json
{
  "request_id": "intent-7f4d...",
  "target": {
    "kind": "sandbox",
    "spec": {
      "image": "docker.io/library/alpine:latest",
      "runtime_class": "smolvm",
      "workdir": "/workspace",
      "network_mode": "egress",
      "resources": {
        "cpu_limit_millis": 2000,
        "memory_limit_bytes": 4294967296
      }
    }
  },
  "ttl_ns": 86400000000000,
  "labels": {
    "world": "dev"
  }
}
```

Response:

```rust
pub struct ControllerSessionOpenResponse {
    pub session_id: SessionId,
    pub status: ControllerSessionStatus,
    pub target_kind: FabricSessionTargetKind,
    pub host_id: HostId,
    pub host_session_id: SessionId,
    pub workdir: String,
    pub supported_signals: Vec<FabricSessionSignalKind>,
    pub created_at_ns: u128,
    pub expires_at_ns: Option<u128>,
}
```

Use the same `session_id` value for the controller session and the P1 host session in P2. This
makes reconciliation and crash recovery simpler. A future version can split controller and provider
session IDs if needed.

### Supported Signals

Supported signal names should be explicit in host, session, and response metadata. This lets the
controller reject invalid signal operations before proxying to a host without making callers
interpret ambiguous lifecycle booleans.

```rust
#[serde(rename_all = "snake_case")]
pub enum FabricSessionSignalKind {
    Quiesce,
    Resume,
    Close,
    TerminateRuntime,
}
```

For smolvm sandbox sessions, P2 should return `["quiesce", "resume", "close"]`. Only include
`terminate_runtime` after host semantics make that distinct from quiesce. For attached-host targets,
`terminate_runtime` must not be listed.

### Controller Exec Request

Controller-facing exec requests should add idempotency and avoid mutually exclusive optional stdin
fields.

```rust
pub struct ControllerExecRequest {
    pub request_id: Option<RequestId>,
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    pub env_patch: BTreeMap<String, String>,
    pub stdin: Option<ExecStdin>,
    pub timeout_ns: Option<u128>,
}

#[serde(tag = "kind", content = "spec", rename_all = "snake_case")]
pub enum ExecStdin {
    Text(String),
    Base64(String),
}
```

P2 may map `ExecStdin::Text` to the current P1 host `stdin_text` field. `ExecStdin::Base64` should
either be decoded and forwarded after host binary stdin support lands, or rejected with
`unsupported_stdin` rather than silently corrupting bytes.

### Host Providers

Host registration and heartbeat should advertise providers as tagged sums:

```rust
#[serde(tag = "kind", content = "spec", rename_all = "snake_case")]
pub enum FabricHostProvider {
    Smolvm(SmolvmProviderInfo),
    AttachedHost(AttachedHostProviderInfo),
}

pub struct SmolvmProviderInfo {
    pub runtime_version: Option<String>,
    pub supported_runtime_classes: Vec<String>,
    pub allowed_images: Vec<String>,
    pub allowed_network_modes: Vec<NetworkMode>,
    pub resource_defaults: ResourceLimits,
    pub resource_max: ResourceLimits,
    pub capacity: ProviderCapacity,
}

pub struct ProviderCapacity {
    pub max_sessions: Option<u64>,
    pub active_sessions: u64,
    pub max_concurrent_execs: Option<u64>,
    pub active_execs: u64,
}
```

P2 may synthesize `SmolvmProviderInfo` from the current P1 `/v1/host/info` response if
`fabric-host` does not yet advertise provider records natively. The controller database should
store provider records in the tagged shape anyway.

### Host Selectors

Host selectors should also be tagged sums:

```rust
#[serde(tag = "kind", content = "spec", rename_all = "snake_case")]
pub enum HostSelector {
    HostId(HostId),
    Pool(String),
    Labels(BTreeMap<String, String>),
}
```

For P2, selectors primarily matter for attached-host target round trips and fake-host tests. The
smolvm sandbox scheduler can ignore selectors until a sandbox target grows an explicit selector
field.

### Lifecycle Signals

Controller-facing signal requests should distinguish machine lifecycle from session lease lifecycle:

```rust
pub struct ControllerSignalSessionRequest {
    pub request_id: Option<RequestId>,
    pub signal: FabricSessionSignal,
}

#[serde(tag = "kind", content = "spec", rename_all = "snake_case")]
pub enum FabricSessionSignal {
    Quiesce(QuiesceSignal),
    Resume(ResumeSignal),
    Close(CloseSignal),
    TerminateRuntime(TerminateRuntimeSignal),
}
```

Mapping to the P1 host API:

- `quiesce` maps to host `SessionSignal::Quiesce`.
- `resume` maps to host `SessionSignal::Resume`.
- `close` maps to host `SessionSignal::Close`.
- `terminate_runtime` should only be sent to providers that advertise support for it.

Current P1 note: host `Terminate` currently behaves like quiesce. The controller should not rely on
that as runtime destruction semantics.

## Controller HTTP API

### Health and Introspection

```text
GET /healthz
GET /v1/controller/info
GET /v1/hosts
GET /v1/hosts/{host_id}
GET /v1/hosts/{host_id}/inventory
GET /v1/sessions?label=key:value
```

`/v1/controller/info` returns controller version, SQLite path, heartbeat timeout, default TTL, and
auth mode.

The host and session list endpoints are for development and reconciliation inspection. They are not
the AOS adapter's primary surface.

### Adapter or Client to Controller

These are the P3 adapter-facing routes:

```text
POST /v1/sessions
GET  /v1/sessions/{session_id}
PATCH /v1/sessions/{session_id}/labels
POST /v1/sessions/{session_id}/exec
POST /v1/sessions/{session_id}/signal
GET  /v1/sessions/{session_id}/fs/file?path=...
PUT  /v1/sessions/{session_id}/fs/file
POST /v1/sessions/{session_id}/fs/edit
POST /v1/sessions/{session_id}/fs/apply_patch
POST /v1/sessions/{session_id}/fs/grep
POST /v1/sessions/{session_id}/fs/glob
POST /v1/sessions/{session_id}/fs/mkdir
POST /v1/sessions/{session_id}/fs/remove
GET  /v1/sessions/{session_id}/fs/stat?path=...
GET  /v1/sessions/{session_id}/fs/exists?path=...
GET  /v1/sessions/{session_id}/fs/list_dir?path=...
```

The controller should preserve the P1 host filesystem surface so that `fabric-cli host ...` and
`fabric-cli controller ...` share filesystem subcommands while still using explicit API surfaces.

`POST /v1/sessions/{session_id}/exec` must support `Accept: application/x-ndjson` and stream events
as they arrive from the host. Non-streaming JSON can be added later; streaming is required.

`GET /v1/sessions` supports repeated label filters:

```text
GET /v1/sessions?label=world:dev&label=task:build
```

Multiple label filters are ANDed. P2 exact-match filters use `key:value`; existence-only filters
can be added later if needed. The list endpoint reconstructs the response label map from
`session_labels`.

`PATCH /v1/sessions/{session_id}/labels` mutates the normalized label table:

```rust
pub struct SessionLabelsPatchRequest {
    pub set: BTreeMap<String, String>,
    pub remove: Vec<String>,
}

pub struct SessionLabelsResponse {
    pub session_id: SessionId,
    pub labels: BTreeMap<String, String>,
}
```

### Host to Controller

```text
POST /v1/hosts/register
POST /v1/hosts/{host_id}/heartbeat
```

Register request:

```rust
pub struct HostRegisterRequest {
    pub host_id: HostId,
    pub endpoint: String,
    pub providers: Vec<FabricHostProvider>,
    pub labels: BTreeMap<String, String>,
}
```

Register response:

```rust
pub struct HostRegisterResponse {
    pub host_id: HostId,
    pub status: HostStatus,
    pub heartbeat_interval_ns: u128,
    pub controller_time_ns: u128,
}
```

Heartbeat request:

```rust
pub struct HostHeartbeatRequest {
    pub host_id: HostId,
    pub endpoint: Option<String>,
    pub providers: Vec<FabricHostProvider>,
    pub inventory: Option<HostInventoryResponse>,
    pub labels: BTreeMap<String, String>,
}
```

P2 can make `inventory` optional and let the controller call the host's existing
`GET /v1/host/inventory` endpoint after heartbeat. Pushing inventory in the heartbeat is still the
preferred shape because it works better for future deployments where the host can reach the
controller more easily than the controller can reach the host.

### Controller to Host

The controller calls the P1 host API:

```text
GET  {host_endpoint}/healthz
GET  {host_endpoint}/v1/host/info
GET  {host_endpoint}/v1/host/inventory
POST {host_endpoint}/v1/sessions
GET  {host_endpoint}/v1/sessions/{session_id}
POST {host_endpoint}/v1/sessions/{session_id}/exec
POST {host_endpoint}/v1/sessions/{session_id}/signal
... fs routes ...
```

P2 should add bearer auth support to `FabricControllerClient` and `FabricHostClient` so both can
call secured controller or host endpoints.

## SQLite State

Use one SQLite database owned by the controller. Suggested default path:

```text
.fabric-ctrl/controller.sqlite
```

Use integer nanosecond timestamps where possible. SQLite integers are signed 64-bit, so use
`i64` nanoseconds for database columns and reject timestamps that overflow `i64`.

P2 is still pre-release. Schema changes may redesign the SQLite shape directly; compatibility
migrations for earlier P2 scratch databases are not required.

### Tables

`hosts`

```sql
CREATE TABLE hosts (
  host_id TEXT PRIMARY KEY,
  endpoint TEXT NOT NULL,
  status TEXT NOT NULL,
  providers_json TEXT NOT NULL,
  labels_json TEXT NOT NULL,
  last_heartbeat_ns INTEGER,
  created_at_ns INTEGER NOT NULL,
  updated_at_ns INTEGER NOT NULL
);
```

`sessions`

```sql
CREATE TABLE sessions (
  session_id TEXT PRIMARY KEY,
  target_kind TEXT NOT NULL,
  target_json TEXT NOT NULL,
  host_id TEXT NOT NULL,
  host_session_id TEXT NOT NULL,
  status TEXT NOT NULL,
  workdir TEXT,
  supported_signals_json TEXT NOT NULL,
  created_at_ns INTEGER NOT NULL,
  updated_at_ns INTEGER NOT NULL,
  expires_at_ns INTEGER,
  closed_at_ns INTEGER,
  FOREIGN KEY(host_id) REFERENCES hosts(host_id)
);

CREATE INDEX sessions_host_id_idx ON sessions(host_id);
CREATE INDEX sessions_status_idx ON sessions(status);
```

`session_labels`

Session labels are stored only in this normalized table. Do not also store a `labels_json` copy on
`sessions`; that creates two sources of truth and makes label updates ambiguous.

```sql
CREATE TABLE session_labels (
  session_id TEXT NOT NULL,
  key TEXT NOT NULL,
  value TEXT NOT NULL,
  PRIMARY KEY(session_id, key),
  FOREIGN KEY(session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
);

CREATE INDEX session_labels_key_value_idx
  ON session_labels(key, value, session_id);

CREATE INDEX session_labels_session_idx
  ON session_labels(session_id);
```

`execs`

`execs` stores only idempotent execs, meaning controller exec requests that include `request_id`.
Non-idempotent development execs are streamed through the controller without durable exec rows by
default.

```sql
CREATE TABLE execs (
  exec_id TEXT PRIMARY KEY,
  scope TEXT NOT NULL,
  request_id TEXT NOT NULL,
  session_id TEXT NOT NULL,
  host_id TEXT NOT NULL,
  status TEXT NOT NULL,
  request_json TEXT NOT NULL,
  host_exec_id TEXT,
  exit_code INTEGER,
  error_message TEXT,
  started_at_ns INTEGER,
  completed_at_ns INTEGER,
  created_at_ns INTEGER NOT NULL,
  updated_at_ns INTEGER NOT NULL,
  FOREIGN KEY(session_id) REFERENCES sessions(session_id),
  FOREIGN KEY(host_id) REFERENCES hosts(host_id)
);

CREATE UNIQUE INDEX execs_scope_request_id_idx
  ON execs(scope, request_id);

CREATE INDEX execs_session_id_idx ON execs(session_id);
CREATE INDEX execs_status_idx ON execs(status);
```

`exec_events`

```sql
CREATE TABLE exec_events (
  exec_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  event_json TEXT NOT NULL,
  created_at_ns INTEGER NOT NULL,
  PRIMARY KEY(exec_id, seq),
  FOREIGN KEY(exec_id) REFERENCES execs(exec_id)
);
```

`exec_events` is likewise only required for idempotent execs. The controller should not become a
general-purpose terminal log store in P2.

`idempotency`

```sql
CREATE TABLE idempotency (
  scope TEXT NOT NULL,
  request_id TEXT NOT NULL,
  operation TEXT NOT NULL,
  request_hash TEXT NOT NULL,
  status TEXT NOT NULL,
  resource_kind TEXT,
  resource_id TEXT,
  response_json TEXT,
  error_json TEXT,
  created_at_ns INTEGER NOT NULL,
  updated_at_ns INTEGER NOT NULL,
  expires_at_ns INTEGER,
  PRIMARY KEY(scope, request_id)
);

CREATE INDEX idempotency_expires_at_idx ON idempotency(expires_at_ns);
```

`host_inventory`

```sql
CREATE TABLE host_inventory (
  host_id TEXT NOT NULL,
  session_id TEXT NOT NULL,
  inventory_json TEXT NOT NULL,
  observed_at_ns INTEGER NOT NULL,
  PRIMARY KEY(host_id, session_id),
  FOREIGN KEY(host_id) REFERENCES hosts(host_id)
);
```

P2 may store provider, target, lifecycle, request, response, and inventory records as JSON. Session
labels are the exception: they are controller-queryable data and must live in `session_labels` only.
The Rust protocol types remain the source of truth for the JSON shapes. Add indexes for fields only
when the controller actually queries them.

## Idempotency

Idempotency is a P2 responsibility because AOS replay must not re-run external work once the
controller has observed and persisted a terminal result.

### Scope

The idempotency key is `(scope, request_id)`.

For P2, `scope` can be the authenticated principal if auth is enabled, otherwise `"dev"`. P3 can
map this to world/tenant identity.

### Request Hash

Compute `request_hash` from:

- operation kind,
- stable canonical JSON of the semantically relevant request body,
- route identity such as session ID for session-scoped operations.

Rules:

1. First request inserts `status = in_flight`.
2. Same `request_id` and same hash while in flight returns `409 request_in_flight`.
3. Same `request_id` and different hash returns `409 idempotency_key_conflict`.
4. Same `request_id` and completed response returns the stored response or stored NDJSON events.
5. Failed terminal controller errors may be stored and replayed if the host operation definitely
   reached a terminal state.
6. Transient controller errors before host dispatch should release or fail the idempotency record so
   the caller can retry.

### Required Idempotent Operations

P2 must implement durable idempotency for:

- `POST /v1/sessions`,
- `POST /v1/sessions/{session_id}/exec`.

P2 should also accept `request_id` on mutating signal and filesystem operations so P3 does not need
a protocol break. If time is tight, those operations may initially return `501 not_implemented` for
idempotent replay rather than silently ignoring `request_id`.

### Exec Event Replay

For idempotent exec requests, the controller should persist every streamed event in `exec_events`.
When a completed exec is replayed, the controller returns the stored events as NDJSON without
calling the host again.

For in-flight exec replay, P2 may return `409 request_in_flight`; reattach-to-live-stream is not
required.

For non-idempotent exec requests where `request_id` is omitted, P2 should proxy the live stream only
and avoid durable `execs` or `exec_events` writes. A later debug/retention feature can add optional
short-lived exec logging without changing the replay contract.

## Scheduling

P2 scheduling should be deterministic and boring.

Host eligibility:

1. host status is `healthy`,
2. last heartbeat is within `host_heartbeat_timeout_ns`,
3. host advertises a provider matching the requested target kind,
4. provider policy allows requested image, runtime class, network mode, and resources,
5. provider has capacity for at least one more session.

Selection:

```text
sort eligible hosts by (priority, host_id)
choose first
```

P2 does not need bin packing, cost optimization, locality, preemption, or queueing.

For `sandbox` targets, select `smolvm` providers. For `attached_host` targets, select
`attached_host` providers if fake tests register one. In production P2, attached-host requests can
return `unsupported_target` until P10 implements the provider.

## Session Open Flow

Use controller-generated session IDs.

If `request_id` is present:

```text
session_id = sess-<short-hash(scope + request_id)>
```

If `request_id` is absent:

```text
session_id = sess-<uuid-v4>
```

Flow:

1. Validate request and target variant.
2. Acquire idempotency row if `request_id` is present.
3. Select a healthy host/provider.
4. Insert `sessions` row with `status = creating`.
5. Commit before network I/O.
6. Convert controller target to host `SessionOpenRequest`.
7. Call host `POST /v1/sessions` with the controller-generated `session_id`.
8. On success, update session to `ready` or returned status.
9. Store idempotency response.
10. Return controller response.

Crash recovery:

- If the controller crashes after inserting `creating` but before host dispatch, reconciliation can
  mark the session `error` or retry if the idempotency record is still valid.
- If the controller crashes after host open but before updating SQLite, host inventory should reveal
  the session because the controller-generated session ID was sent to the host.

## Exec Flow

Flow:

1. Validate session exists and is assigned to a healthy host.
2. If `request_id` is present, acquire idempotency row and insert `execs` row with
   `status = running`.
4. Call host exec endpoint with `Accept: application/x-ndjson`.
5. For each host event:
   - persist event in `exec_events` only for idempotent execs,
   - forward it to the caller immediately,
   - update terminal exec status on `exit` or `error` only for idempotent execs.
6. For idempotent execs, complete idempotency with stored events and terminal status.

If the downstream client disconnects:

- for requests with `request_id`, the controller should keep reading the host stream until terminal
  if practical, so replay can return the final events;
- for requests without `request_id`, best-effort cleanup is acceptable in P2.

P2 does not require reattaching to a live in-flight stream.

## Filesystem Proxy Flow

The controller looks up `session_id`, resolves the assigned host, and forwards the filesystem RPC to
the host endpoint.

Rules:

1. The controller does not perform path confinement itself; the host remains responsible for
   enforcing workspace confinement.
2. The controller should still enforce session existence, status, and host assignment.
3. Reads and searches can be proxied without idempotency.
4. Mutating filesystem calls should accept `request_id` in the controller protocol even if durable
   replay support lands after initial P2.
5. Host error envelopes are translated into stable controller error envelopes without leaking
   transport details unless useful for debugging.

## Signal Flow

The controller enforces supported signals before dispatch.

Rules:

1. `quiesce` and `resume` require provider/session support.
2. `close` closes the Fabric session and maps to host close for smolvm sessions.
3. `terminate_runtime` is only valid for providers whose sessions list runtime termination in
   `supported_signals`.
4. Attached-host sessions must reject runtime termination with `unsupported_lifecycle`.
5. After a successful signal, update the controller session row with the returned status.

## Host Registration And Heartbeat

`fabric-host` should gain controller registration flags:

```text
--controller-url
--advertise-url
--controller-auth-token-file
--host-auth-token-file
--heartbeat-interval-ns
```

Dynamic host-initiated registration is required for P2. Static controller host config may exist as a
local-development fallback, but scheduler correctness and reconciliation should be built around
registration plus heartbeat.

Host startup flow:

1. Build provider info from local config and `/v1/host/info`.
2. POST `/v1/hosts/register` to the controller.
3. Periodically POST `/v1/hosts/{host_id}/heartbeat`.
4. Include provider capacity and, if cheap enough, inventory snapshot.

Controller heartbeat handling:

1. Authenticate the host.
2. Upsert `hosts` row.
3. Store provider capabilities.
4. Store inventory snapshot if present.
5. Mark host healthy.
6. Trigger lightweight reconciliation for that host.

Heartbeat timeout:

- if `now - last_heartbeat_ns > host_heartbeat_timeout_ns`, mark host `unhealthy`;
- sessions on unhealthy hosts should become `host_unreachable`, not `closed`;
- when the host returns, reconciliation updates session statuses from inventory.

## Reconciliation

Reconciliation runs:

- on controller startup,
- after host registration,
- after heartbeat with inventory,
- periodically in the background.

Algorithm for each healthy host:

1. Load host inventory from heartbeat snapshot or `GET /v1/host/inventory`.
2. Upsert `host_inventory` rows.
3. For each controller session assigned to the host:
   - if inventory has matching session, update status from host status,
   - if inventory lacks matching session and session is not terminal, mark `lost` or
     `host_missing`.
4. For each host inventory session without controller session:
   - record it as `orphaned_host_session`,
   - do not adopt it automatically unless it matches a `creating` session ID.
5. For stale `creating` sessions:
   - if host inventory contains the session, recover to the observed status,
   - otherwise mark `error` after a startup grace period.

Controller-specific statuses may need to extend P1 statuses:

- `creating`
- `ready`
- `quiesced`
- `closing`
- `closed`
- `error`
- `lost`
- `host_unreachable`
- `orphaned_host_session`

Do not change P1 host statuses solely for controller bookkeeping; map them at the controller
boundary.

## Authentication

P2 should implement bearer-token authentication with local-dev escape hatches.

Controller accepts:

- adapter/client token for session and fs APIs,
- host token for registration and heartbeat.

Host daemon accepts:

- controller token for controller-to-host calls.

Required controller flags:

```text
--bind
--db-path
--adapter-auth-token-file
--host-auth-token-file
--allow-unauthenticated-loopback
--host-heartbeat-timeout-ns
--default-session-ttl-ns
--max-session-ttl-ns
```

Required host additions:

```text
--auth-token-file
--allow-unauthenticated-loopback
```

Defaults:

- bind controller on `127.0.0.1:8788`,
- SQLite path `.fabric-ctrl/controller.sqlite`,
- unauthenticated loopback allowed for local development,
- non-loopback bind requires auth tokens,
- heartbeat timeout 30 seconds,
- default session TTL 24 hours,
- no mTLS in P2.

## Error Model

Use JSON error envelopes:

```json
{
  "code": "no_healthy_host",
  "message": "no healthy smolvm host can satisfy sandbox target"
}
```

Stable P2 controller error codes:

- `invalid_request`
- `unauthorized`
- `forbidden`
- `not_found`
- `conflict`
- `request_in_flight`
- `idempotency_key_conflict`
- `unsupported_target`
- `unsupported_lifecycle`
- `unsupported_stdin`
- `no_healthy_host`
- `host_unavailable`
- `host_timeout`
- `host_error`
- `reconciliation_error`
- `runtime_error`

Recommended HTTP mapping:

- `400`: invalid request,
- `401`: missing or invalid auth,
- `403`: forbidden by policy,
- `404`: missing host/session/path,
- `409`: conflict, in-flight idempotency, invalid state,
- `422`: unsupported target or lifecycle variant,
- `502`: host error while proxying,
- `503`: no healthy host or host unavailable,
- `504`: host timeout,
- `500`: controller bug or database failure.

## Direct Smoke Flow

Expected local development flow:

```bash
target/debug/fabric-controller \
  --bind 127.0.0.1:8788 \
  --db-path .fabric-ctrl/controller.sqlite \
  --allow-unauthenticated-loopback
```

In another terminal:

```bash
LIBKRUN_BUNDLE="$PWD/third_party/smolvm-release/lib" \
DYLD_LIBRARY_PATH="$PWD/third_party/smolvm-release/lib" \
SMOLVM_AGENT_ROOTFS="$PWD/third_party/smolvm-release/agent-rootfs" \
target/debug/fabric-host \
  --bind 127.0.0.1:8791 \
  --state-root .fabric-host \
  --host-id local-dev \
  --controller-url http://127.0.0.1:8788 \
  --advertise-url http://127.0.0.1:8791
```

Verify host registration:

```bash
target/debug/fabric controller hosts
```

Open a session through the controller:

```bash
target/debug/fabric controller open \
  --request-id smoke-open-1 \
  --image alpine:latest \
  --label smoke=true \
  --net
```

Run a streaming exec through the controller:

```bash
target/debug/fabric controller exec <session-id> \
  --request-id smoke-exec-1 \
  --timeout-secs 20 \
  -- sh -lc 'printf hello; printf err >&2'
```

Repeat the same exec request and verify the controller replays stored events without calling the
host again.

The smoke flow should also cover:

1. write a file through the controller,
2. read it back through the controller,
3. quiesce and resume through the controller,
4. close through the controller,
5. restart the controller and verify session reconciliation from host inventory.
6. inspect controller API docs at `http://127.0.0.1:8788/docs`.

## Tests

Unit tests:

- [ ] tagged sum JSON round trips for targets, providers, selectors, and signals,
- [ ] request hash stability for idempotency,
- [ ] same request ID and different body returns `idempotency_key_conflict`,
- [ ] same in-flight request returns `request_in_flight`,
- [ ] completed session open replay returns stored response,
- [ ] completed exec replay returns stored NDJSON events,
- [ ] scheduler chooses first healthy eligible host deterministically,
- [ ] scheduler rejects no-capacity and policy-mismatched hosts,
- [ ] heartbeat timeout marks hosts unhealthy,
- [ ] reconciliation maps host inventory to controller session statuses,
- [ ] orphaned host sessions are recorded but not adopted automatically,
- [ ] host error envelopes map to controller error envelopes.

Integration tests with fake hosts:

- [x] host register and heartbeat upsert SQLite rows,
- [x] controller opens a sandbox session on a fake smolvm provider,
- [x] controller proxies streaming exec and persists events,
- [x] controller proxies filesystem read/write/list/stat/exists,
- [x] controller proxies signal and updates session status,
- [x] controller restart reloads SQLite state and reconciles fake host inventory,
- [ ] auth rejects missing/invalid tokens on non-loopback or auth-required config.

Gated integration tests with real smolvm host:

- [x] controller-driven open session with one real `fabric-host`,
- [x] exec stdout/stderr streams through controller before command exit,
- [x] quiesce/resume/close works through controller,
- [x] repeated idempotent open and exec do not create duplicate host work.

Real smolvm tests should use the existing opt-in pattern:

```text
FABRIC_SMOLVM_E2E=1 cargo test -p fabric-controller --test controller_smolvm_e2e
dev/fabric/test-controller-smolvm-e2e.sh
```

Tests must skip cleanly if smolvm is unavailable or the host lacks hypervisor support.

## Implementation Order

1. [x] Add `fabric-controller` crate, controller config, health endpoint, and SQLite open/migration.
2. [x] Add controller protocol types for tagged targets, providers, selectors, supported signals,
   and controller open/signal requests.
3. [ ] Add bearer auth support to the shared HTTP client and controller/host routers.
4. [x] Add host registration and heartbeat endpoints backed by SQLite.
5. [x] Add optional `fabric-host` registration and heartbeat loop.
6. [x] Add deterministic scheduler for sandbox targets and smolvm providers.
7. [x] Add controller `POST /v1/sessions` with durable idempotency and host dispatch.
8. [x] Add session status and reconciliation from host inventory.
9. [x] Add controller exec streaming proxy with event persistence and replay.
10. [x] Add filesystem proxy routes.
11. [x] Add signal proxy routes and lifecycle capability checks.
12. [x] Add fake-host controller integration tests.
13. [x] Add gated real-smolvm controller e2e test and smoke docs.
14. [x] Add OpenAPI docs for controller and host APIs.

## Definition Of Done

P2 is complete when:

1. [x] `fabric-controller` can run locally without AOS crates.
2. [x] A `fabric-host` instance can register and heartbeat with the controller.
3. [x] The controller persists hosts, sessions, idempotent execs/events, inventory, and idempotency
   records in SQLite.
4. [x] A direct client can open a smolvm-backed sandbox session through the controller.
5. [x] Exec stdout/stderr streams through the controller as NDJSON while the command is still
   running.
6. [x] Repeating completed idempotent open and exec requests does not re-run host work.
7. [x] Filesystem and signal RPCs proxy through the controller to the assigned host.
8. [x] Controller restart reconciliation recovers session status from host inventory.
9. [x] Controller unit and fake-host integration tests cover idempotency, scheduling, and
   reconciliation.
10. [x] The controller API is stable enough for P3 `FabricHostBackend` work to start.
