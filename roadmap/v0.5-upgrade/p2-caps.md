# p2-caps: Capability System (Current State + Work Remaining)

## TL;DR
Capabilities are wired and enforced in the kernel at enqueue time (grant exists, cap type matches effect kind, reducer slot binding exists). However, cap **params**, **budgets**, and **expiry** are not enforced, and adapters do not see cap data. The system is structurally correct but not yet a complete authorization/budget model.

---

## Diamond Invariants (Design Spine)

1) **One authoritative authorizer, one deterministic transaction**
   - Kernel authorization must be the single decision point:
     - canonicalize params -> cap check -> policy check -> journal decisions -> enqueue or deny
   - Any mutation (budget reservation/settlement, counters, approvals) must be part of that same deterministic, journaled transaction.

2) **Caps and policy stay orthogonal in shape**
   - Caps answer: "can ever" and return a structured decision.
   - Policies answer: "should now" and return a structured decision.
   - The kernel composes them in fixed order: cap -> policy.

3) **Every decision is explainable and replayable**
   - Allowed/denied outcomes must be derivable from the journal alone.
   - Cap denials should record *what constraint failed*.
   - Budget usage must be ledgered (reservation + settlement).

---

## What Caps Are For (Conceptual Model)
Caps are **static, typed grants** that authorize which effect kinds a world may emit, plus **parameters** that constrain how those effects may be used (e.g., allowed HTTP hosts, LLM models, max body size). The invariant is:

- If a plan/reducer can emit an effect, it must present a **cap grant** whose **cap type** matches that effect kind and whose **params** authorize that specific use.

Caps must be deterministic, auditable, and enforced **before** enqueue.

---

## Current Implementation (What Works Today)

### 1) Cap grants are resolved and schema-validated at manifest load
- `manifest.defaults.cap_grants` are resolved against `defcap`.
- Grant params are validated against the cap schema and canonicalized.
- Each grant is associated with a **cap type**.

Relevant code:
- `crates/aos-kernel/src/capability.rs`

### 2) Cap type must match effect kind at enqueue
- On any effect enqueue, the kernel resolves the grant by name and verifies that its **cap type** matches the effect kind's expected cap type.

Relevant code:
- `crates/aos-kernel/src/effects.rs`
- `crates/aos-kernel/src/capability.rs`

### 3) Plans must declare required caps
- `plan.required_caps` are enforced at manifest load; missing grants reject the manifest.

Relevant code:
- `crates/aos-kernel/src/world.rs`

### 4) Reducer cap slots are wired
- Reducer micro-effects include optional `cap_slot` (default: `"default"`).
- The kernel maps the slot to a cap grant via `manifest.module_bindings`.

Relevant code:
- `crates/aos-wasm-abi/src/lib.rs`
- `crates/aos-kernel/src/world.rs`

### 5) Effect intents carry only `cap_name`
- `EffectIntent` contains `cap_name` but not cap params or cap type.
- Adapters see only the intent (kind, params, cap_name, hash).

Relevant code:
- `crates/aos-effects/src/intent.rs`
- `crates/aos-host/src/adapters/*`

---

## What Is Not Implemented (Gaps)

1) **Cap params are not enforced against effect params**
   - No runtime comparison of cap constraints (hosts/models/limits) against effect inputs.

2) **Budgets are not enforced**
   - Cap grant budgets (tokens/bytes/cents) are parsed but not decremented or checked.

3) **Expiry is not enforced**
   - `expiry_ns` is ignored at runtime.

4) **No ledgered budget state**
   - There is no journaled reservation/settlement or replayable cap usage.

5) **Adapters do not validate caps**
   - Adapters never see cap params, so they cannot enforce constraints. (This is fine as long as the kernel is authoritative.)

---

## Where Enforcement Must Live

**Primary enforcement is in the kernel before enqueue.**

- Determinism and auditability require a single, canonical authorizer.
- Adapters may still apply safety checks, but must not be authoritative.

If adapters ever need visibility for defense-in-depth, pass **immutable identifiers** (e.g., `cap_type`, `cap_grant_hash`) for logging, not for decision-making.

---

## Cap-Type Enforcement Interface (Proposed Shape)

Make cap enforcement a first-class interface per cap type:

- `validate_constraints(cap_params, effect_kind, effect_params) -> ok | error{path, reason}`
- `estimate_reservation(cap_params, effect_kind, effect_params) -> {tokens?, bytes?, cents?}`
- `settle_from_receipt(cap_params, effect_kind, receipt) -> {tokens?, bytes?, cents?}`

This keeps cap logic centralized and makes it easy to add new cap types without leaking policy or budget logic into adapters.

### Cap Handler Registry (Clarification)
Implement cap enforcement via a registry keyed by `cap_type`, similar to effect adapter registration. This keeps the kernel authoritative while making cap logic modular and extensible.

---

## Cap Param Shapes (Updated for v0.5)
Cap params can be refactored for this milestone. Proposed shapes:

- **sys/http.out@1**: `{ hosts?: set<text>, schemes?: set<text>, methods?: set<text>, ports?: set<nat>, path_prefixes?: set<text> }`
- **sys/llm.basic@1**: `{ providers?: set<text>, models?: set<text>, max_tokens?: nat, tools_allow?: set<text> }`
- **sys/blob@1**: `{}` (no constraints in v0.5)
- **sys/timer@1**: `{}` (no constraints in v0.5)
- **sys/secret@1**: `{ aliases?: set<text>, binding_ids?: set<text> }`
- **sys/governance@1**: `{}`
- **sys/query@1**: `{ scope?: text }`

Notes:
- Missing/empty fields mean "no restriction"; non-empty fields are allowlists/ceilings.
- HTTP enforcement can parse URLs in the kernel for now; future option is structured HTTP params to avoid parsing.

---

## Budgets: Two-Phase Reserve -> Settle (Diamond Upgrade)

To avoid both oversubscription and over-counting:

1) **Reserve at enqueue**
   - Compute a conservative upper bound reservation.
   - Deny if insufficient budget.
   - Journal the reservation.

2) **Settle at receipt**
   - Compute actual usage from receipt (tokens/bytes/cents where available).
   - Refund unused reservation or charge additional if actual exceeds reserve.
   - Journal the settlement.

Practical v1 reservations:
- `llm.generate`: reserve `max_tokens` (optionally a conservative prompt estimate); settle on receipt usage.
- `blob.put`: reserve known size (CAS metadata if available); settle on receipt size.
- `cost_cents`: reserve 0 unless bounded; settle from receipt cost if provided.

---

## Expiry Requires Deterministic "Now"

Expiry must not read wallclock in the kernel. Options:

- **Logical time**: journal height / deterministic epoch counter.
- **Trusted time receipts**: a timer adapter produces signed receipts, and the kernel updates a deterministic "now" state.

Either way, expiry is enforced against a deterministic, journaled value.

---

## Minimal "Working Cap System" Requirements

1) **Cap param enforcement**
   - Enforce cap constraints against effect params at enqueue.
   - Journal allow/deny decisions with structured reasons (journal is the only log).

2) **Two-phase budget ledger**
   - Reservation at enqueue; settlement at receipt.

3) **Expiry enforcement**
   - Use deterministic "now" and deny expired caps.

4) **Audit trail**
   - Journal allow/deny decisions with reasons and budget deltas.

---

## Proposed Minimal Use-Cases (Tests/Examples)

1) **HTTP allowlist**
   - Cap params include `hosts` (and optional `methods`, `schemes`, `ports`, `path_prefixes`).
   - Allowed host -> ok, disallowed host -> denied (with reason).

2) **LLM model allowlist + token budget**
   - Cap params include `allowed_models` and budget.
   - Disallowed model -> denied.
   - Budget reserved at enqueue, settled on receipt.

3) **Blob constraints**
   - No cap constraints in v0.5 (cap exists to match effect kind only).

---

## Governance Relationship

- Caps are manifest-level objects; governance patches add/modify defcaps and grants.
- Shadow runs load the patched manifest, so cap wiring is exercised during shadow.
- Governance-level caps (who can propose what) are separate and still TODO.

---

## Build Order (Minimal to Meaningful)

1) Cap param enforcement
2) Journaled cap decisions (allow/deny + reasons)
3) Two-phase budget ledger (reserve/settle)
4) Expiry enforcement with deterministic "now"
5) Cap-type interface stabilization + tests/fixtures

---

## Summary of Required Work

1) Implement cap param enforcement at enqueue.
2) Implement budget reservation + settlement with ledgered deltas.
3) Implement deterministic expiry enforcement.
4) Journal cap decisions with rationale.
5) Add minimal use-case tests and replay checks.

Once these exist, caps are a real security and budget control surface, not just wiring.
