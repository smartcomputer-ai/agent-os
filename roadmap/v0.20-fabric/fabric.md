# v0.19 Fabric Follow-On

This folder is intentionally downstream of `roadmap/v0.18-execution/`.

It assumes the core owner/executor seam already exists:

- worlds record durable open external work first,
- executors run somewhat independently of the owner loop,
- continuations re-enter only through owner admission,
- start eligibility and stale-start rules are already defined.

What remains here is the fabric-specific layer built on top of that seam:

- host sessions,
- execution classes,
- detached host/container/vm/sandbox execution,
- artifact and log surfaces,
- secret-provider integration.

## Background

`v0.17-kafka` established the log-first hosted seam, Kafka/S3 recovery model, route-first hosted
runtime, and embedded/local convergence work.

`v0.18-execution` now carries the core external-execution contract.
What this folder intentionally finishes is the fabric-side product and execution layer that uses
that contract to run work on VMs, containers, and sandboxes.

## Assumes

- `roadmap/v0.18-execution/p1-open-effect-lifecycle-and-owner-executor-seam.md`
- `roadmap/v0.18-execution/p2-start-eligibility-expiry-and-staleness.md`
- `roadmap/v0.18-execution/p3-runtime-execution-strategies.md`

## Requirements

### Now
- get access to a unix machine to run commands
- run/exec commands as rpc or jobs (jobs later)
- rpc: immediate feedback, stream back outputs (important if longer running), multiple calls at the same time allowed
- sessions run from a few seconds/minutes to days/weeks
- quiesce sessions allowed
- currently not meant to deploy software permanently, more as an agent dev scratchpad
- check out repositories
- work with rust, python, node, go, etc
- install new/missing tools inside a session (e.g. install via curl or apt-get)
- semi-permanent volumes: not the end of the world if lost. do not have to survive sessions
- sessions are standalone (no cross-session communication needed)
- no docker inside sessions itself
- local sessions (access local machine), but not always enabled

### Later:
- SSH connections/sessions
- deploy software permanently (pods, services, etc)
- volumes survive sessions, can be re-used across sessions
- volumes can be shared between sessions
- desktop sessions
- browser sessions
- sessions can communicate (e..g one session runs a db, the other backen, the other the UI)
- docker inside sessions (e.g. run docker compose to test stuff)
- jobs: one job runs against any given session at a time, can be long running, can take a while


## Fabric Runtime Options
- Containers
- Firecracker VMs
- Normal VMs

Hundreds to thousands of sessions will exist at any time. But since they are mainly used for dev. Many sessions do not always need to be live.

Fabric should coordinate the sessions and underlying compute resources. Sessions should probably _not_ be k8s pods, but rather be managed by the fabric coordinator. A host VM that hosts sessions, _could_ be a k8s resource in the hosted system.

Right now, I'm thinking we should start with container based sessions and then later expand to firecracker. Also, I'm not sure how hard it would be to support SSH sessions in order to connect to any unix server?

## Not In Scope Here

This folder no longer defines:

- the open-effect lifecycle,
- stale-start semantics,
- attached versus detached runtime strategy labels,
- the shared owner/executor seam itself.

Those belong in `roadmap/v0.18-execution/`.
