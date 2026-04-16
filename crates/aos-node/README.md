# aos-node

`aos-node` is the library crate for the daemonized node runtime.

The crate is split into:

- `src/model/` for shared node identities and domain types
- `src/control/` for the unified control facade, HTTP surface, and request/response DTOs
- `src/model/backends.rs` for internal distributed runtime backend contracts

## What belongs here

These concerns belong in the unified node runtime:

- world/storage identity types and validation helpers
- shared DTOs for world creation, ingress, commands, receipts, checkpoints, snapshots, and runtime views
- shared control-facing API DTOs
- internal backend/runtime helpers and contracts
- secret binding/version contracts needed by node control flows

## What stays out

These concerns should not enter the shared node protocol crate:

- FoundationDB keyspace layout or runtime helpers
- FoundationDB-backed persistence implementations
- the old lease/inbox/segment coordination seam
- deployment binary/config wiring
- authored-world filesystem UX
- shared execution primitives in `src/execution/`

## Current scope

This crate now owns:

- the shared node models used by the runtime
- the shared `/v1` control-facing DTO surface in `src/control/`
- the internal backend/runtime seam in `src/model/backends.rs`
- the shared execution/runtime primitives in `src/execution/`
