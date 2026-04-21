# P10: Attached Host Daemon

**Priority**: P10  
**Status**: Tentative  
**Effort**: High  
**Depends on**:
- `roadmap/v0.20-fabric/fabric.md`
- P2 Fabric controller target/provider model

## Goal

Add an attached-host mode for Fabric.

In this mode, `fabric-host` is installed on an existing computer and registers that computer with
the Fabric controller. The computer may be a bare-metal server, an externally deployed VM, or an
externally deployed Kubernetes pod. Fabric does not create or own the computer lifecycle in this
mode; it provides scoped session access to it.

This should use the same controller, session, exec, signal, and filesystem API as managed smolvm
sandbox sessions. The scheduler and lifecycle semantics differ by target/provider kind.

## Naming

Use **attached host** for the product/protocol concept.

Keep **`fabric-host`** as the daemon binary name. Avoid calling this component an "agent" in the
protocol because AgentOS already uses agent for the actor doing work.

Suggested terms:

- Fabric host: any registered computer endpoint.
- Smolvm provider: a host provider that can create Fabric-managed sandbox VMs.
- Attached-host provider: a host provider that exposes the existing computer itself.
- Attached session: a scoped lease/workspace on an attached host.

## Design Rules

Use tagged sum types with variant-specific records. Do not represent target or provider variants as
optional-field bags.

Suggested controller target shape:

```text
FabricSessionTarget =
  sandbox(FabricSandboxTarget)
  attached_host(FabricAttachedHostTarget)
```

Suggested host provider shape:

```text
FabricHostProvider =
  smolvm(SmolvmProviderInfo)
  attached_host(AttachedHostProviderInfo)
```

Suggested host selector shape:

```text
HostSelector =
  host_id(HostId)
  pool(String)
  labels(map<string, string>)
```

The preferred JSON wire shape is `kind` plus `spec`, for example:

```json
{
  "target": {
    "kind": "attached_host",
    "spec": {
      "selector": {
        "kind": "host_id",
        "spec": "build-server-01"
      },
      "workspace_policy": "per_session_directory",
      "workdir": "/srv/fabric/workspaces/world-123",
      "user": "fabric"
    }
  }
}
```

## Semantics

An attached-host session is a lease on an existing computer, not a request to create a computer.

Session operations:

- `open`: create or attach to a scoped workspace and admit future exec/fs calls.
- `exec`: run an argv command on the attached host, usually confined to the session workspace.
- `fs.*`: operate on the attached workspace through direct host filesystem access.
- `quiesce`: stop admitting new execs for the session, if supported.
- `resume`: re-admit execs for a quiesced session, if supported.
- `close`: close the session lease and clean up the workspace according to policy.
- `terminate_runtime`: unsupported for attached hosts because Fabric does not own the machine.

The controller should expose supported signal names in session and host metadata so callers can
distinguish unsupported operations from transient runtime failures.

## Attached Target Fields

Suggested first `FabricAttachedHostTarget` fields:

- `selector`: tagged host selector.
- `workspace_policy`: `per_session_directory`, `existing_directory`, or `ephemeral_tmp`.
- `workdir`: requested working directory or workspace root.
- `user`: optional OS user or execution identity.
- `env`: base session environment.
- `ttl_ns`: session lease TTL.
- `labels`: tenant/world/session attribution.

The attached-host target should not include image, runtime class, CPU limit, memory limit, or network
mode fields unless a specific attached provider can enforce them. Provider-specific enforcement
belongs in provider capability records, not in generic optional fields.

## Provider Capabilities

Suggested first `AttachedHostProviderInfo` fields:

- `host_id`
- `hostname`
- `os`
- `arch`
- `workspace_roots`
- `default_workspace_root`
- `supported_workspace_policies`
- `exec_concurrency_limit`
- `supported_users`
- `default_user`
- `supports_quiesce`
- `supports_resume`
- `supports_fs`
- `supports_exec`
- `labels`

The controller scheduler should match attached-host targets by selector and capability. For P10,
the deterministic policy can remain simple: first healthy matching host with available concurrency.

## Host Daemon Shape

`fabric-host` should gain provider configuration rather than a separate binary:

```text
fabric-host --provider smolvm
fabric-host --provider attached-host
fabric-host --provider smolvm --provider attached-host
```

Internally, do not bolt attached-host fields onto the smolvm runtime path. Add a provider/backend
boundary that can support both:

```text
HostSessionProvider =
  SmolvmSessionProvider
  AttachedHostSessionProvider
```

Both providers should implement the same host-facing operations after session allocation:

- open session,
- session status,
- exec stream,
- signal session,
- inventory,
- filesystem operations.

## State Model

The controller remains authoritative for session allocation, idempotency, and attached-host leases.

The attached host daemon may keep recoverable local workspace markers similar to the P1 smolvm
marker files, but it should not add a second scheduling/idempotency database. After daemon restart,
it reports attached-session inventory from marker files and workspace directories; the controller
reconciles that with SQLite state.

Suggested attached host data root:

```text
{state_root}/attached/
  sessions/
    {session_id}/
      workspace/
      tmp/
      logs/
      fabric-attached-session.json
```

## Security Posture

Attached host mode is operationally useful but less isolated than one microVM per session. It should
default to conservative local-machine semantics:

- disabled unless explicitly configured,
- controller-to-host bearer auth or mTLS required,
- run as a dedicated non-root OS user by default,
- no host control socket exposure,
- no SSH agent forwarding by default,
- path confinement under configured workspace roots,
- reject symlink escapes,
- argv execution without shell construction by the daemon,
- explicit allowlist of execution users if user switching is supported,
- stable errors for unsupported lifecycle/resource isolation requests.

Do not claim strong multi-tenant isolation for attached host mode until there is a concrete OS-level
sandboxing story for the target platform.

## Non-Goals

P10 does not implement:

- provisioning bare-metal servers, VMs, or Kubernetes pods,
- Kubernetes scheduling or pod lifecycle management,
- SSH-backed hosts,
- permanent service deployment,
- Docker-in-session support,
- strong multi-tenant sandboxing on arbitrary existing machines,
- cross-session networking.

## Implementation Order

1. Extend protocol/controller tests so attached-host targets and provider records round-trip as
   tagged sums.
2. Add attached-host provider configuration to `fabric-host`.
3. Add attached-host provider registration and heartbeat payloads.
4. Implement attached session open/status/inventory with workspace markers.
5. Implement direct host filesystem operations under the attached workspace root.
6. Implement exec streaming against the attached workspace.
7. Implement close/quiesce/resume according to provider capabilities.
8. Add controller smoke tests with one attached host and no smolvm runtime.

## Definition Of Done

P10 is complete when:

1. An existing Unix machine can register with the Fabric controller as an attached host.
2. The controller can open an attached session through the same `/v1/sessions` API used for
   sandbox sessions.
3. Exec and filesystem operations work through the same session endpoints.
4. Closing an attached session does not terminate or power-manage the underlying machine.
5. Unsupported lifecycle operations return stable capability errors.
6. Restart reconciliation preserves controller authority and reports local attached-session
   inventory accurately.
