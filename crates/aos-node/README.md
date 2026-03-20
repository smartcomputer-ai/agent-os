# aos-node

`aos-node` is the shared seam for daemonized node runtime code.

It is intentionally narrower than `aos-fdb`:

- keep backend-neutral node protocol here
- keep the shared hot-world open/replay helpers here
- keep the hosted persistence adapter here
- keep FoundationDB implementation details out
- keep hosted deployment wiring out
- keep single-world runtime concerns in `aos-runtime`

## What belongs here

These concerns need to be shared by local and hosted node backends:

- universe/world identity types
- backend-neutral persistence errors and conflicts
- universe-scoped CAS contracts
- world journal, inbox, snapshot, and segment persistence contracts
- durable command submission and polling contracts
- world and universe admin metadata contracts
- shared node HTTP/control surface
- shared hot-world open/replay helpers
- hosted persistence adapter for persisted-world opens
- hosted-only coordination and queue traits for distributed nodes
- secret binding/version contracts needed by both backends

## What stays out

These concerns should not enter the shared node protocol crate:

- FoundationDB keyspace layout
- FoundationDB transaction/runtime helpers
- FoundationDB-backed persistence implementations
- hosted deployment binary/config wiring
- hosted-only operational config
- local-node runtime home layout
- authored-world filesystem UX
- `aos-runtime` single-world execution internals

## Naming

The extracted protocol uses backend-neutral names for node concerns:

- `HostedRuntimeStore`
- `WorldAdminStore`
- `UniverseStore`
- `SecretStore`
- `NodeWorldRuntimeInfo`
- `WorldRecord`
- `SecretBindingSourceKind::NodeSecretStore`

## Current scope

This crate now owns:

- the shared node protocol that was previously defined inside `aos-fdb`
- the in-memory backend used for tests and backend-neutral execution
- the shared `/v1` control surface
- the hosted persisted-world adapter/open path extracted out of `aos-runtime`

`aos-fdb` remains responsible for:

- FoundationDB runtime and persistence implementations
- FDB keyspace layout
- CAS/storage implementation details
- projection/materialization helpers tied to the backend implementation
