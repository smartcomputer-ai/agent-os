# aos-node

`aos-node` is the shared seam for daemonized node runtime code.

For the v0.17 Kafka cutover, that seam is now split into:

- `src/model/` for shared node identities and domain types
- `src/api/` for control-facing request/response DTOs
- `src/planes/` for internal distributed runtime and plane contracts

## What belongs here

These concerns need to be shared by local and hosted node backends:

- world/storage identity types and validation helpers
- shared DTOs for world creation, ingress, commands, receipts, checkpoints, snapshots, and runtime views
- shared control-facing API DTOs
- internal plane/runtime helpers and contracts
- secret binding/version contracts needed by local and hosted control flows

## What stays out

These concerns should not enter the shared node protocol crate:

- FoundationDB keyspace layout or runtime helpers
- FoundationDB-backed persistence implementations
- the old hosted lease/inbox/segment coordination seam
- hosted deployment binary/config wiring
- hosted-only operational config
- local-node runtime home layout
- authored-world filesystem UX
- `aos-runtime` single-world execution internals

## Current scope

This crate now owns:

- the surviving shared node models used by local and hosted runtimes
- the shared `/v1` control-facing DTO surface in `src/api/`
- the internal plane/runtime seam in `src/planes/`
