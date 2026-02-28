# P5: Blob Effects CAS-Direct I/O and Journal Ref-Only

**Priority**: P5  
**Status**: Proposed  
**Date**: 2026-02-28

## Goal

Make `blob.put` / `blob.get` scale for heavy CAS interaction by removing blob bytes
from the journal path.

Target outcome:

1. journal only refs/metadata for blob effects,
2. read/write blob bytes directly to CAS,
3. preserve replay determinism via CAS dependency closure.

## Current State (Why Change)

Today blob bytes are duplicated into journal records:

1. `blob.put` intent params include raw `bytes` (`BlobPutParams.bytes`) and are
   journaled as `EffectIntentRecord.params_cbor`.
2. `blob.get` receipt payload includes raw `bytes` (`BlobGetReceipt.bytes`) and
   is journaled as `EffectReceiptRecord.payload_cbor`.
3. Blob legacy receipt projection also re-embeds requested/receipt payloads.
4. Host adapters already use CAS for actual blob storage/read.

References:

- `crates/aos-effects/src/builtins/mod.rs:36`
- `crates/aos-effects/src/builtins/mod.rs:64`
- `crates/aos-kernel/src/world/mod.rs:1095`
- `crates/aos-kernel/src/world/mod.rs:1111`
- `crates/aos-kernel/src/receipts.rs:251`
- `crates/aos-kernel/src/receipts.rs:271`
- `crates/aos-host/src/adapters/blob_put.rs:27`
- `crates/aos-host/src/adapters/blob_get.rs:27`

## Design Decision

For blob effects, CAS is the source of truth for bytes. Journal is the source
of truth for references, ordering, and decisions.

That means:

1. blob bytes are never carried in blob effect intent/receipt schemas,
2. blob effects carry hash refs + metadata only,
3. runtime code reads/writes bytes directly to CAS by ref.

## Blob Contract Changes (v2 Schemas)

## `blob.put`

Replace byte-carrying params with ref-carrying params.

1. `sys/BlobPutParams@2`:
   - `blob_ref: HashRef` (required)
   - `size: nat` (required)
   - `refs?: list<HashRef>`
2. `sys/BlobPutReceipt@2` remains ref/metadata only (`blob_ref`, `edge_ref`, `size`).

## `blob.get`

Make get receipt ref-only.

1. `sys/BlobGetParams@2` remains `blob_ref`.
2. `sys/BlobGetReceipt@2`:
   - `blob_ref: HashRef`
   - `size: nat`
   - no `bytes` field.

## Receipt projection

1. Add `sys/BlobPutResult@2` and `sys/BlobGetResult@2` using ref-only
   requested/receipt schemas.
2. Do not project raw bytes into these events.

## Kernel/Host Behavior

## Write path (`blob.put`)

1. Workflow/runtime submits blob bytes to CAS directly (by hash).
2. Effect intent uses ref-only params (`blob_ref`, `size`, `refs?`).
3. Enqueue + cap/policy + journal operate on ref-only canonical CBOR.

## Read path (`blob.get`)

1. Effect intent carries `blob_ref` only.
2. Host/kernel verifies/read metadata from CAS.
3. Receipt is ref-only (`blob_ref`, `size`) and journaled as such.
4. If workflow needs bytes, it performs explicit CAS read by `blob_ref`
   (runtime API path, not byte-carrying effect receipt).

## Replay and Determinism

1. Replay never re-executes external effects.
2. Replay uses journaled refs and metadata exactly as recorded.
3. Any required CAS blob missing during replay is a deterministic hard fault
   (`missing_cas_dependency` class).
4. World export/import tooling must include transitive CAS deps referenced by
   blob effect intents/receipts/events.

## Journal Model Impact

No generic journal-record format change is required for this slice.

1. Existing `params_cbor` / `payload_cbor` fields remain.
2. For blob effects, those CBOR payloads become ref-only by schema.
3. Optional follow-up: generic large-payload externalization (`*_ref`) for
   non-blob effects.

## Migration Plan

## Phase 5.1: Dual-read, v2-write

1. Introduce `@2` blob schemas and defs.
2. Accept v1/v2 on read during transition.
3. Emit/journal v2 ref-only contracts on new worlds.

## Phase 5.2: Runtime API and SDK alignment

1. Add explicit workflow/runtime CAS read helper by `blob_ref`.
2. Update SDK helpers and examples to avoid relying on byte-carrying
   blob effect receipts.

## Phase 5.3: Remove v1 byte-carrying path

1. Disable `BlobPutParams@1` / `BlobGetReceipt@1` in strict mode.
2. Remove legacy blob projection that embeds bytes.

## Risks

1. Migration breakage for workflows that parse v1 blob receipt payload bytes.
2. Replay failures if CAS dependency closure is incomplete.
3. Operator confusion if v1/v2 coexist without clear diagnostics.

## Deliverables / DoD

1. `blob.put` and `blob.get` effect journals are ref-only (no inline bytes).
2. `sys/Blob*Result@2` events are ref-only.
3. Replay-or-die still holds with explicit missing-CAS fault behavior.
4. Smoke/integration fixtures cover heavy blob workloads without journal bloat.
