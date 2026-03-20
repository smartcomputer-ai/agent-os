# P6: World Control Mailbox (Authoritative Writes Via Control Ingress)

**Priority**: P6  
**Effort**: High  
**Risk if deferred**: Medium-High (the hosted API can read and enqueue events, but cannot yet execute privileged world-scoped commands without direct holder routing)  
**Status**: Implemented

## Goal

Add the authoritative write path for hosted world operations that must execute inside the leased single-world engine.

This milestone is intentionally narrow:

1. direct and projected reads are covered by P5.1 and P5.2,
2. direct admin/storage writes are covered by P5.2,
3. P6 covers only world-scoped authoritative writes through a durable command mailbox.

Examples:

- governance propose/shadow/approve/apply,
- pause/archive/delete world lifecycle transitions,
- future privileged world control commands.

## Design Stance

- Do not build a generic cluster message bus.
- Do not depend on direct request forwarding to the current lease holder for correctness.
- Reuse the hosted inbox as the durable submission path.
- Finish hosted `Command` ingress as a typed sibling to `DomainEvent` and `Receipt`.
- Let the existing world-ready / lease-acquire / inbox-drain machinery deliver and execute control commands.
- Keep application/worldflow mutations that are already modeled as domain events on `DomainEvent` ingress.
- Do not route governance through fake domain events; treat it as privileged control handled by the world engine/kernel path.

This keeps the mutation authority model coherent:

- only the lease holder mutates authoritative world state,
- the control plane submits durable requests,
- workers execute those requests under the same existing guarded path.

It also preserves the existing semantic split:

- domain events are for workflow routing and application-level facts,
- control commands are for privileged world-scoped operations handled by the engine,
- internal effects such as `governance.*` remain the programmable in-world path for workflows to request those same privileged operations.

## Why P6 Exists

Some writes are not just persistence mutations:

- they depend on current interpreted world state,
- they must serialize with other world execution,
- they must respect the single-holder lease boundary.

These are not good fits for:

- direct FDB admin writes,
- synchronous RPC to an arbitrary worker,
- a separate command scheduler.

The cleanest solution is a per-world command mailbox.

## Command Mailbox Model

### Submission

The control role receives an HTTP command request and persists it as hosted command ingress:

```text
CommandIngress {
  command_id: String,
  command: String,
  actor?: String,
  payload: CborPayload,
  submitted_at_ns: u64
}
```

Important boundary:

- `CommandIngress` is a hosted/runtime transport envelope, not an AIR builtin `defschema`.
- Control payloads may still be AIR-typed and schema-validated where that is useful.
- Reusing existing builtin payload schemas is preferred over inventing parallel command-only shapes when the command already has a canonical world semantic form.
- The public API should expose the same opaque identifier as `command_id`; that same value is used in `CommandIngress`.

Submission rules:

- request must be idempotent by `command_id`,
- enqueue marks the world ready,
- submission returns immediately with durable acknowledgment and command ID,
- submission does not require knowing the active lease holder.

### Delivery

Workers already:

- scan ready worlds,
- acquire or renew leases,
- drain hosted inbox,
- append authoritative journal entries under guarded mutation paths.

The command mailbox should reuse that exact loop.

This means:

- no second wakeup path,
- no second delivery scheduler,
- no separate "command worker" concept.

### Execution

Only the active lease holder executes the control command.

Execution rules:

- command is decoded by `command` plus typed payload contract,
- command is handled inside the world engine/control path,
- resulting authoritative mutations are committed under the normal guarded lease boundary,
- result status is persisted durably.

Journaling rule:

- raw command ingress is not itself the canonical journal record,
- execution writes the normal authoritative journal records for the operation performed,
- commands that need a domain event as their semantic outcome may inject that domain event during execution,
- command result storage provides durable request/response tracking separate from the journal.

## Command Families

Suggested first command families:

- governance:
  - `gov-propose`
  - `gov-shadow`
  - `gov-approve`
  - `gov-apply`
- lifecycle:
  - `world-pause`
  - `world-archive`
  - `world-delete`

These are commands, not domain events.

They should be:

- schema-validated,
- auditable,
- explicitly privileged,
- distinct from application-level domain traffic.

Command encoding stance:

- command should be modeled as a closed enum in implementation/spec,
- on the wire and at rest it should serialize as a stable string token,
- that preserves type safety without exposing language-specific enum encodings.

Payload typing stance:

- Governance commands should reuse the existing builtin governance payload schemas and kernel handlers where possible:
  - `sys/GovProposeParams@1`
  - `sys/GovShadowParams@1`
  - `sys/GovApproveParams@1`
  - `sys/GovApplyParams@1`
- Lifecycle commands may use hosted control-plane request types unless/until there is a strong need to make them AIR-level typed world semantics.
- `sys/WorkspaceCommit@1` is intentionally not a control command. It remains a normal domain event routed to `sys/Workspace@1`.

## Command Status Model

Public status polling should use a single command record and a single opaque identifier.

Do not introduce a second generic "operation" abstraction for these world-scoped commands.

Instead:

- the public API returns `command_id`,
- that same ID is used in the mailbox,
- there is one durable status record for the async command.

Every async world command needs a durable command record keyed by command ID.

Suggested shape:

```text
CommandRecord {
  command_id: String,
  command: String,
  status: "queued" | "running" | "succeeded" | "failed",
  submitted_at_ns: u64,
  started_at_ns?: u64,
  finished_at_ns?: u64,
  journal_height?: u64,
  manifest_hash?: String,
  result_payload?: CborPayload,
  error?: ApiErrorBody
}
```

Why these fields matter:

- `status` supports polling and operators,
- `journal_height` gives precise audit anchoring,
- `manifest_hash` is important for governance/apply flows,
- `result_payload` keeps the mailbox useful beyond fire-and-forget.

### Result Storage

`CommandRecord` should live in the per-world hosted keyspace alongside inbox/journal/runtime state:

- `u/<u>/w/<w>/commands/by_id/<command_id> -> CommandRecord`

Optional helper indexes:

- `u/<u>/w/<w>/commands/by_status/<status>/<submitted_at_ns>/<command_id> -> ()`
- `u/<u>/w/<w>/commands/gc/<finished_bucket>/<command_id> -> ()`

Storage rules:

- write the initial `queued` result in the same transaction that enqueues `InboxItem::Command(CommandIngress { ... })`,
- the leased worker updates the same `CommandRecord` record to `running` when execution starts,
- on completion the worker updates the record to `succeeded` or `failed` with terminal metadata,
- inbox consumption is tracked by the inbox cursor and is independent from command-result retention,
- command-result retention should be longer-lived than inbox retention so polling/audit survives inbox compaction.

Relationship to P5.3:

- P5.3 may still use the more general term "operation" for wider control-plane work,
- P6 should use the more precise term `command` throughout for world-scoped leased execution,
- public polling for P6 world commands should read the world-scoped `CommandRecord`.

## HTTP Shape

Do not expose the raw command mailbox as a public generic route.

The mailbox is an internal execution substrate, not the API resource model.

Public routes should be intent-specific:

```text
GET  /v1/universes/{universe_id}/worlds/{world_id}/commands/{command_id}
```

The hosted API may also expose stable convenience routes for governance on top of the mailbox:

```text
POST /v1/universes/{universe_id}/worlds/{world_id}/governance/propose
POST /v1/universes/{universe_id}/worlds/{world_id}/governance/shadow
POST /v1/universes/{universe_id}/worlds/{world_id}/governance/approve
POST /v1/universes/{universe_id}/worlds/{world_id}/governance/apply
```

Those routes are the public authority path.

Internally they submit mailbox commands, but clients should not need to know that.

Their request bodies should reuse the existing governance parameter schemas semantically, even if the outer HTTP envelope is ordinary JSON and the hosted `CommandIngress` envelope is not itself part of AIR.

The same pattern should be used for world lifecycle admin routes:

```text
POST   /v1/universes/{universe_id}/worlds/{world_id}/pause
POST   /v1/universes/{universe_id}/worlds/{world_id}/archive
DELETE /v1/universes/{universe_id}/worlds/{world_id}
```

Those routes should:

- create or update a `CommandRecord`,
- enqueue the corresponding typed lifecycle command,
- return `202 Accepted`,
- leave final world status transition to the active lease holder.

Workspace note:

- workspace tree editing/root construction stays in the direct hosted control/storage plane from P5.2,
- workspace commit remains `POST /events` with schema `sys/WorkspaceCommit@1`,
- P6 does not add a `workspace-commit` control command.

Submission response:

- `202 Accepted`
- body includes `command_id`, initial `queued` status, and polling URL

Polling response:

- returns the persisted `CommandRecord`

## Why This Is Better Than Holder Routing

Compared with direct "find the current holder and forward the request":

- no dependency on holder reachability for correctness,
- no handoff race when lease changes,
- no need for a separate worker-address discovery protocol,
- no second authority path,
- works even when no worker currently holds the world.

Compared with a generic message bus:

- world-scoped ordering is natural,
- delivery semantics reuse the existing hosted inbox model,
- operational surface stays small.

This also solves the multi-control-instance problem for lifecycle finalization:

- no `GET` request needs to mutate state,
- no control node needs to run a competing finalizer loop,
- the authoritative lease holder performs the terminal transition (`pausing -> paused`, `archiving -> archived`, `deleting -> deleted`) when runtime conditions are satisfied.

## Constraints

- Commands are asynchronous by default.
- Strong idempotency is required.
- The mailbox remains world-scoped only.
- Universe/global admin operations stay in the direct control plane from P5.2.
- Existing domain-event-based world operations should not be migrated to control without a semantic reason.
- This milestone does not attempt true synchronous command execution APIs.

## Non-Goals (P6)

- Restored-world read sessions.
- Latest-live read semantics.
- Shell or advanced interactive read APIs.
- Direct request forwarding to the active holder as a correctness dependency.
- A cluster-wide general-purpose bus.

## Scope (Now)

### [x] 1) Finish hosted typed `Command` ingress

Implement hosted support for typed command ingress alongside:

- `DomainEvent`
- `Receipt`

Notes:

- `CommandIngress` remains a hosted runtime protocol type, not a builtin AIR schema.
- Add durable enqueue/dequeue support and worker drain support for `Command`.
- While touching hosted inbox-kind support in the worker, also implement processing of `TimerFired` ingress.
- Do not change the existing `DomainEvent` path for `sys/WorkspaceCommit@1`.

### [x] 2) Add durable command result storage

Implement:

- command records,
- idempotency handling,
- polling lookup by command ID.

### [x] 3) Add leased worker execution for control commands

Implement:

- decode/dispatch for typed control commands,
- authoritative execution inside the world engine,
- durable result persistence.

Dispatch rules:

- governance control commands should reuse the same kernel governance operations used by the existing local control path and internal `governance.*` effect handlers,
- lifecycle commands execute directly in the world/lifecycle control path,
- commands may emit canonical journal records or inject canonical domain events as appropriate for the operation.

### [x] 4) Add HTTP submission and polling endpoints

Implement:

- command submit,
- command status/result read,
- governance submit façades over typed mailbox commands,
- lifecycle submit façades for pause/archive/delete world operations,
- error/status mapping.

Explicitly out of scope for this milestone:

- replacing `/events` for workspace commit,
- inventing AIR builtin command schemas for the outer `CommandIngress` envelope,
- routing governance through workflow subscriptions as domain traffic.

## Exit Criteria

P6 is complete when:

1. The hosted API can submit governance and lifecycle commands without direct holder routing.
2. Those commands are durably queued through hosted command ingress.
3. Leased workers execute them under the normal guarded authority boundary.
4. Clients can poll durable command status through a single `GET /v1/universes/{universe_id}/worlds/{world_id}/commands/{command_id}` API.
5. Governance commands reuse the existing typed governance payload contracts and do not travel as fake domain events.
6. `sys/WorkspaceCommit@1` continues to travel via `DomainEvent` ingress rather than the command mailbox.
7. No restored-world read path is required to ship the main hosted control plane.

Implementation note:

- While delivering P6 inbox execution support, also close the adjacent hosted worker gap for `TimerFired` inbox processing so command and timer ingress no longer remain the only undrained hosted inbox kinds.
