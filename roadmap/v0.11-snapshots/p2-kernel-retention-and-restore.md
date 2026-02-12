# P2: Kernel Baseline Restore and Time-Based Retention

**Priority**: P2
**Effort**: Medium/High
**Risk if deferred**: High (cannot safely bound disk growth before infra)
**Status**: Proposed

## Goal

Implement kernel/runtime behavior for baseline-aware restore and minimum viable
retention: time-based pruning of hot journal/snapshot artifacts without full GC.

## Scope

This sprint is intentionally local-first and deterministic. It establishes the
same behavior required later in distributed infra.

## Decision Summary

1) Baseline + tail is the authoritative restore path.
2) Introduce retention policies that are simple and safe:
   - keep latest baseline
   - keep snapshots younger than a configured age
   - keep journal tail younger than a configured age and above baseline
3) Enforce receipt-horizon safety before baseline promotion/truncation actions.
4) Add planning mode before destructive retention actions.

## Kernel/Host Changes

### 1. Baseline-aware restore path

- On open/recover:
  - load latest baseline metadata
  - load baseline snapshot state
  - replay tail entries in order (`>= baseline.height`)
- Fail closed if storage layout violates baseline contract.

### 2. Receipt horizon enforcement

- Add validation hook used by baseline promotion and retention execution.
- Baseline/truncation must be rejected if receipts could still arrive for
  intents below horizon.

### 3. Time-based retention policy (MVP)

Introduce host/kernel policy config (exact placement TBD):

- `min_baselines_to_keep` (default `1`)
- `snapshot_max_age`
- `journal_hot_max_age`
- `retention_grace_period`

Execution behavior:

- Never remove active baseline.
- Never remove entries >= current baseline height.
- Only prune artifacts older than age + grace checks.

### 4. Retention planner

Add non-destructive planner command/path:

- lists candidate snapshots/journal segments/artifacts
- prints reason each candidate is eligible
- emits deterministic plan output for audit and CI

## CLI/Ops Surface

- `aos snapshot baseline <snapshot_ref>` (or equivalent control command)
- `aos retention plan`
- `aos retention run`
- `aos retention check`

`plan` must exist before `run` is enabled by default.

## Tests

- Replay-or-die from baseline + tail.
- Reject baseline promotion when receipt horizon is unsafe.
- Retention never deletes required baseline/tail data.
- Deterministic retention plan output for same world state.

## DoD

- Baseline restore path is default and tested.
- Time-based retention works in local FS store with strict safety checks.
- Operators can run `plan` and `run` with auditable output.

## Non-Goals

- CAS mark/sweep deletion.
- Cross-world/shared-CAS accounting.
- Orchestrator/leases/universe routing.
