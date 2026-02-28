# P5: Blob Effects CAS-Direct I/O and Journal Ref-Only

**Priority**: P5  
**Status**: Complete  
**Date**: 2026-02-28

## Goal

Remove large blob bytes from journal storage without changing the runtime effect/receipt contracts.

Target outcome:

1. `blob.put` / `blob.get` stay on `@1` schemas and behavior,
2. journal stores refs/metadata for large blob payload bytes instead of inline bytes,
3. replay loads externalized payload bytes from CAS deterministically,
4. snapshot+journal roots remain sufficient for future GC reachability.

## Clarification (What Stays the Same)

This slice is an **in-place storage-model change**, not a contract-version change.

1. `sys/BlobPutParams@1` and `sys/BlobGetReceipt@1` remain unchanged.
2. Workflow/runtime behavior remains unchanged: workflows can still consume `blob.get` bytes.
3. Adapter input/output contracts remain unchanged.

Only journal persistence changes.

## Current State (Why Change)

Today raw blob bytes are duplicated into journal records:

1. `blob.put` params include `bytes` and are persisted inline in `EffectIntentRecord.params_cbor`.
2. `blob.get` receipt includes `bytes` and is persisted inline in `EffectReceiptRecord.payload_cbor`.
3. This causes journal bloat for heavy blob workloads even though bytes already live in CAS.

## Design Decision

For blob-heavy payloads, CAS is source of truth for bytes; journal is source of truth for ordering/decisions and refs.

Execution path keeps full payloads in-memory for adapter and workflow delivery, but journal persistence externalizes the large CBOR payload bytes.

## Journal Externalization Model

### Scope (P5)

Externalize only:

1. `blob.put` **intent params** CBOR,
2. `blob.get` **receipt payload** CBOR.

### Record shape changes

Extend journal records with ref metadata:

1. `EffectIntentRecord` adds:
   - `params_ref?: hash`
   - `params_size?: nat`
   - `params_sha256?: hash`
2. `EffectReceiptRecord` adds:
   - `payload_ref?: hash`
   - `payload_size?: nat`
   - `payload_sha256?: hash`

Rules:

1. If `*_ref` is present, replay MUST hydrate bytes from CAS using that ref.
2. When externalized, inline `params_cbor` / `payload_cbor` are not authoritative and may be empty/minimal.
3. If `*_ref` is absent, replay uses inline bytes (legacy/non-externalized path).

## Kernel/Host Behavior

### Write path (`blob.put` intent journaling)

1. Kernel canonicalizes `BlobPutParams@1` as today.
2. Before appending `EffectIntentRecord`, kernel writes canonical params CBOR to CAS.
3. Journal record stores `params_ref` metadata and does not store full inline bytes.

### Write path (`blob.get` receipt journaling)

1. Receipt remains `BlobGetReceipt@1` in execution path.
2. Before appending `EffectReceiptRecord`, kernel writes canonical receipt payload CBOR to CAS.
3. Journal record stores `payload_ref` metadata and does not store full inline bytes.

### Replay path

1. Replay reads journal records.
2. If `params_ref` / `payload_ref` present, replay loads bytes from CAS and verifies size/hash metadata.
3. Hydrated bytes are then processed identically to current inline replay behavior.
4. Missing or mismatched CAS dependency is a deterministic hard fault (`missing_cas_dependency`).

## Determinism and Faulting

1. Replay never re-executes external effects.
2. Replay behavior is fully determined by journal order + CAS objects referenced by journal metadata.
3. Missing CAS object for externalized payload is fatal and deterministic.

## GC Readiness (Aligned with `spec/07-gc.md`)

Future GC must retain externalized journal payload blobs.

Required reachability contract:

1. `params_ref` and `payload_ref` are typed `hash` fields in journal nodes.
2. Mark phase traversal over retained journal records MUST follow these refs.
3. Therefore, externalized payload blobs are reachable from snapshot+journal roots and cannot be collected while needed for replay.
4. World export/import MUST include transitive CAS closure of externalized journal refs.

Operationally:

1. A retained baseline at height `H` implies replay needs journal tail `>= H`.
2. GC retention must keep all externalized payload blobs referenced by that retained tail.
3. Truncated (non-retained) tails may release their externalized payload blobs once unreachable.

## Migration Plan (In-Place)

### Phase 5.1: Journal ref fields + dual-read replay

1. Add `*_ref/*_size/*_sha256` fields to journal records.
2. Replay supports both inline and ref-based records.
3. Writer externalizes the P5 blob paths by default.

### Phase 5.2: Tooling closure and diagnostics

1. Export/import include externalized journal payload CAS closure.
2. Add diagnostics for missing externalized dependencies.

### Phase 5.3: Optional generalization

1. Reuse same externalization mechanism for other large effect payloads.

## Risks

1. Incomplete export/import CAS closure causing replay faults.
2. Incorrect fallback rules between inline vs `*_ref` replay.
3. GC implementation later ignoring journal `*_ref` fields.

## Deliverables / DoD

1. `blob.put` / `blob.get` effect+receipt schemas remain `@1` and behavior unchanged.
2. Journal no longer stores blob-heavy bytes inline for P5 paths.
3. Replay hydrates externalized payload bytes from CAS and preserves behavior.
4. Missing externalized CAS object fails deterministically (`missing_cas_dependency`).
5. Externalized journal payload refs are explicitly reachable from snapshot+journal for future GC.

## Completion Notes (2026-02-28)

1. `EffectIntentRecord` now carries optional `params_ref/params_size/params_sha256`; `EffectReceiptRecord` now carries optional `payload_ref/payload_size/payload_sha256`.
2. Writer externalization is enabled for the P5 scope:
   - `blob.put` intent params are externalized to CAS and journaled by ref.
   - `blob.get` receipt payload is externalized to CAS and journaled by ref.
3. Replay is dual-read and deterministic:
   - if ref metadata is present, replay hydrates from CAS and validates size/hash;
   - if ref metadata is absent, replay uses inline journal CBOR.
4. Missing externalized CAS dependencies deterministically hard-fail with `missing_cas_dependency`.
5. Integration coverage added in `crates/aos-host/tests/blob_journal_externalization_integration.rs`:
   - verifies `blob.put` intent externalization;
   - verifies `blob.get` receipt externalization and replay failure when CAS dependency is absent.
6. Current export/import tooling remains manifest-focused in this repository; explicit runtime journal-tail CAS-closure export is a follow-up when runtime state/journal export is in scope.
