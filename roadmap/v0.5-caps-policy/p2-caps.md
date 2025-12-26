# p2-caps: Capability System (Current State + Work Remaining)

## TL;DR
Capabilities are wired and enforced in the kernel at enqueue time (grant exists, cap type matches effect kind, reducer slot binding exists). Cap **params**, **budgets**, **expiry**, and cap decisions are now enforced/journaled. Cap semantics now live in pure enforcer modules (with builtin allow-all fallback). Remaining gaps: settle ABI wiring (CapSettleInput/Output), broader host-level integration/replay coverage, and spec updates for cap decision records + deterministic time.

---

## Diamond Invariants (Design Spine)

1) **One authoritative authorizer, one deterministic transaction**
   - Kernel authorization must be the single decision point:
     - canonicalize params -> cap check -> policy check -> journal decisions -> enqueue or deny
   - Any mutation (budget reservation/settlement, counters, approvals) must be part of that same deterministic, journaled transaction.
   - The enforcer never makes the final allow/deny decision. It only returns semantic constraint results and resource requirements; the kernel performs expiry + ledger checks and records the final decision.

2) **Caps and policy stay orthogonal in shape**
   - Caps answer: "can ever" by returning constraints + reserve requirements.
   - Policies answer: "should now" and return a structured decision.
   - The kernel composes them in fixed order: cap -> policy.

3) **Every decision is explainable and replayable**
   - Allowed/denied outcomes must be derivable from the journal alone.
   - Cap denials should record *what constraint failed*.
   - Budget usage must be ledgered (reservation + settlement).
   - Journal entries must pin determinism: enforcer module identity, intent/grant identity, and enforcer output (or hash).
   - Reservations are per-intent and idempotent: a receipt can only settle the reservation it created.

---

## What Caps Are For (Conceptual Model)
Caps are **static, typed grants** that authorize which effect kinds a world may emit, plus **parameters** that constrain how those effects may be used (e.g., allowed HTTP hosts, LLM models, max body size). The invariant is:

- If a plan/reducer can emit an effect, it must present a **cap grant** whose **cap type** matches that effect kind and whose **params** authorize that specific use.

Caps must be deterministic, auditable, and enforced **before** enqueue.

### Cap Grants vs DefCaps (Clarification)
**Defcaps** define the *type* of capability: cap type, param schema, and enforcer module.  
**Cap grants** are concrete, named instances of a defcap with fixed params/budget/expiry.

Key rules:
- Plans and reducers reference **cap grants** (grant names), not defcap names.
- Enforcers do **not** mint grants; they evaluate a request against an existing grant.
- A grant’s `cap` field points to the defcap that defines its schema + enforcer.

Put differently: defcap = template, grant = instantiated authority, enforcer = checker.

### Call-Trace: Grant Params → Enforcer Check (Fetch-Notify)
This is the concrete data path for the `example.com` allowlist:

```
manifest.defaults.cap_grants["cap_http_fetch"].params
  -> resolve_grant(): validate + canonicalize params to CBOR
  -> CapabilityGrant.params_cbor
  -> enqueue_effect(): cap_constraints_and_reserve(...)
  -> CapCheckInput.cap_params = params_cbor
  -> sys/CapEnforceHttpOut@1 decodes cap_params.hosts
  -> compare URL host from effect_params.url against hosts allowlist
```

So the enforcer doesn’t “know” about `example.com` magically; it is fed the grant
params every time, in canonical CBOR form, and compares them to the effect’s URL.

### Cap Reference Validation (How It Should Work)
Cap references must be validated against **grants**, not defcaps. Validation should enforce:

1) **Grant existence**
   - Every `emit.cap` in a plan, every `required_caps`, every module `cap_slots` binding,
     and every `secret.allowed_caps` entry must reference a **grant name** in
     `manifest.defaults.cap_grants`.

2) **Grant → defcap correctness**
   - Each grant’s `cap` must reference a defcap in `manifest.caps`.
   - Grant params must validate against the defcap schema (already enforced in kernel load).

3) **Effect kind ↔ cap type match (via the grant)**
   - For each `emit_effect` step, look up its grant, then the grant’s defcap,
     and verify `defcap.cap_type` matches the effect kind’s required cap type.
   - This check should use grants as the primary lookup key; defcaps are only templates.

4) **Uniqueness + consistency**
   - Grant names must be unique.
   - A single grant may be referenced in multiple plans/modules, but all uses must be
     compatible with its defcap cap type.

This makes the authoring model consistent with runtime: **grants are the capability boundary**, and defcaps are the templates.

### Cap Grants: Practical Improvements (Ergonomics + Audit)
Two small changes make grant usage more evolvable and auditable:

1) **Plan-level cap slots (mirror reducer slots)**
   - Today: reducers use `cap_slot` → manifest binds slots to grants; plans reference grants directly.
   - Proposed: allow plans to declare `cap_slot: "http_default"` and add
     `manifest.plan_bindings[plan_name].slots.http_default = "grant_name"`.
   - Benefit: governance can swap grants without touching plan defs; same plan can be wired
     differently across worlds.

2) **Stable grant hash**
   - Keep human-friendly names, but compute:
     `grant_hash = sha256(cbor({defcap_ref, params_cbor, budget, expiry}))`.
   - Journal the hash alongside decisions. This makes “same name, changed meaning”
     detectable and gives adapters/logs a stable identifier without exposing params.

---

## Design Smell (Current Trajectory)
Previously, cap semantics lived in the kernel. A `defcap` only defined param **shape**, while the **meaning** of those params was hardcoded (hosts mean URL host allowlists, max_tokens mean ceilings, etc.). That made caps a closed-world enum disguised as open strings, and would block dynamic adapter/cap addition.

This is now addressed by pure enforcer modules, with builtin allow-all fallback for simple caps.

The diamond invariant already points to the fix: the **kernel is the authoritative transaction boundary**, but the logic executed inside that boundary does not have to be hardcoded. It only needs to be deterministic, pinned, and journaled.

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

### 6) Effect params are canonicalized before policy
- Effect params are canonicalized via the effect schema, then secret variants are normalized.
- This happens before any policy decision, and would also be before cap enforcement.

Relevant code:
- `crates/aos-kernel/src/effects.rs`

### 7) Cap enforcers are wired via pure modules
- `defcap.enforcer` references a pure module; the kernel invokes it during authorization.
- sys enforcers live in `aos-sys` (`sys/CapEnforceHttpOut@1`, `sys/CapEnforceLlmBasic@1`).

Relevant code:
- `crates/aos-kernel/src/cap_enforcer.rs`
- `crates/aos-kernel/src/effects.rs`
- `crates/aos-sys/src/bin/cap_enforce_http_out.rs`
- `crates/aos-sys/src/bin/cap_enforce_llm_basic.rs`

---

## Gap Status (as of p2 implementation)

1) **Cap params are enforced against effect params**
   - DONE: runtime comparison of cap constraints (hosts/models/limits) against effect inputs.

2) **Budgets are enforced**
   - DONE: grant budgets are reserved at enqueue and settled on receipt.

3) **Expiry is enforced**
   - DONE: `expiry_ns` checked against deterministic `logical_now_ns`.

4) **Ledgered budget state exists**
   - DONE: reservation/settlement stored and cap decisions journaled.

5) **Adapters do not validate caps**
   - REMAINS: adapters never see cap params; kernel remains authoritative.

6) **Enforcer modules are wired into auth**
   - DONE: kernel invokes pure enforcers when present.

7) **Settle ABI is wired**
   - REMAINS: `sys/CapSettleInput@1` / `sys/CapSettleOutput@1` schemas exist, but no settle invocation yet.

8) **Host-level replay coverage**
   - REMAINS: expand integration tests that replay cap decisions across journal/snapshot boundaries.

9) **Per-intent reservation records**
   - REMAINS: represent reservations as first-class records keyed by `intent_hash`, not only aggregate counters.

10) **Release path for stuck intents**
   - REMAINS: deterministic "release reservation" path (cancel/expiry/governance) + introspection tooling.

11) **Plan-level cap slots**
   - REMAINS: allow plans to use slots that are bound to grants via `manifest.plan_bindings`.

12) **Stable grant hash in journals**
   - REMAINS: compute + record a `grant_hash` alongside cap decisions.

13) **Policy decision journaling**
   - REMAINS: journal allow/deny/require-approval decisions for replay/audit.

---

## Where Enforcement Must Live

**Primary enforcement is in the kernel before enqueue.**

- Determinism and auditability require a single, canonical authorizer.
- Adapters may still apply safety checks, but must not be authoritative.

If adapters ever need visibility for defense-in-depth, pass **immutable identifiers** (e.g., `cap_type`, `cap_grant_hash`) for logging, not for decision-making.

---

## Proposed Direction: Cap Enforcers as Deterministic Modules
Status: DONE (wired; sys enforcers implemented in `aos-sys`).

### Key idea
Make cap enforcement a **deterministic, pinned module** that the kernel runs inside the authorizer transaction. The kernel stays a small interpreter/journaler, while new caps ship as data + modules.

This aligns with the "solid state interpreter" goal:
- No per-cap kernel code.
- New cap types are `defcap` + module artifacts.
- Shadow runs can execute the same enforcer logic for prediction/audit.

### Use the existing `module_kind: "pure"` ABI
The schema already supports deterministic pure modules and the kernel can invoke them. The missing piece is to wire them into authorization. The ABI remains:

- `module_kind: "pure"`
- `run(input_bytes) -> output_bytes` using canonical CBOR in/out (schema-pinned).

This is the reusable substrate for cap enforcers, policy engines, and param normalizers.

**Pure module determinism profile:** pure modules are deterministic and side-effect-free (no wallclock, randomness, or ambient I/O). They may receive state snapshots as explicit inputs and return deltas as outputs.

### Make `defcap` carry an enforcer
Add a required enforcer module reference:

```json
{
  "$kind": "defcap",
  "name": "sys/http.out@1",
  "cap_type": "http.out",
  "schema": { ... },
  "enforcer": { "module": "sys/CapEnforceHttpOut@1" }
}
```

Adding a new cap type becomes “ship a new `defcap` + module”, not “edit the kernel”.

---

## Cap Enforcer ABI (Proposed)
Status: enqueue check implemented; settle ABI not wired yet.

Make cap enforcement a first-class, deterministic ABI:

1) **enqueue check** (constraints + reservation estimate)
2) **receipt settle** (actual usage)

Conceptual input:

```cbor
CapCheckInput = {
  cap_def: Name,
  grant_name: text,
  cap_params: bytes,        // canonical CBOR
  effect_kind: text,
  effect_params: bytes,     // canonical CBOR
  origin: { kind: "plan"|"reducer", name: Name },
  logical_now_ns: nat
}
```

Output:

```cbor
CapCheckOutput = {
  constraints_ok: bool,
  deny?: { code, message },           // only if constraints_ok=false
  reserve_estimate: map<text,nat>
}
```

### Tagged union wrapper (single-input ABI)
Pure modules expose a single `run(input_bytes) -> output_bytes`. To support **both**
check + settle in one module, wrap them in a tagged union:

```cbor
CapEnforcerInput = variant {
  Check: CapCheckInput,
  Settle: CapSettleInput
}

CapEnforcerOutput = variant {
  Check: CapCheckOutput,
  Settle: CapSettleOutput
}
```

The kernel chooses the variant (`Check` at enqueue, `Settle` on receipt). The module
returns the matching variant. This keeps a single ABI surface while allowing
separate logic for reserve vs settle.

### Settle ABI (Proposed)

Settle input includes receipt + reservation; output returns actual usage deltas.

```cbor
CapSettleInput = {
  cap_def: Name,
  grant_name: text,
  cap_params: bytes,          // canonical CBOR
  effect_kind: text,
  effect_params: bytes,       // canonical CBOR (from enqueue)
  origin: { kind: "plan"|"reducer", name: Name },
  logical_now_ns: nat,
  intent_hash: bytes,         // 32 bytes
  reserve_estimate: map<text,nat>,
  receipt: {
    status: text,             // "ok" | "error"
    adapter_id: text,
    payload: bytes,           // receipt payload CBOR
    cost_cents?: nat
  }
}

CapSettleOutput = {
  usage: map<text,nat>,
  violation?: { code, message }  // e.g., usage > reserve or receipt mismatch
}
```

**Semantics:**
- `usage` is authoritative for ledger settlement (kernel still applies deltas).
- `violation` indicates receipt/usage contract failure (policy for handling is kernel-owned).
- Enforcers may return empty usage maps when a cap has no budget dimensions.

### Ledger Invariants (Correctness Over Time)
Two-phase accounting only stays correct if reservations are tracked per intent, not just
as aggregate counters. Model reservations as first-class records keyed by `intent_hash`:

```
Reservation = {
  intent_hash,
  grant_name,
  enforcer_ref,           // hash or {manifest_hash, module_name}
  reserve: map<text,nat>,
  status: "reserved"|"settled"|"released"
}
```

Derived aggregates:
- `reserved_total[dim] = sum(reserve[dim]) over status="reserved"`
- `spent_total[dim]` is monotone-increasing based on settle usage.

This makes settlement idempotent: if a duplicate receipt arrives, the kernel can skip
settle when status != "reserved". It also makes ledger replay robust.

### Pin Enforcer Identity for Settlement
When a reservation is created, record the enforcer identity used for the check
(module hash or `{manifest_hash, module_name}`). On settle, use the same identity
to interpret receipts. This prevents semantic drift across upgrades while intents
are in flight.

### Intent Outcomes (Must Be Explicit)
Every reserved intent should end in exactly one of:
1) **Receipt arrives → settle**
2) **Explicit release → reservation released** (cancel/expiry/governance)
3) **No receipt → reservation remains reserved** (correct but operationally visible)

This implies a deterministic release path (even if vNext) and introspection tooling
to see outstanding reservations by grant + dimension.

### Settlement Semantics for Failure/Timeout
Receipts should always settle:
- If receipt lacks usage for a dimension, treat usage as `0` for that dimension.
- Always release the reservation, even on error/timeout.
- Policy may still count attempts separately; cap budgets should charge actual usage.

### Wiring Plan (Kernel)
1) **Capture enqueue context**: store `effect_params` (canonical CBOR), `cap_params`, and `origin` in `CapReservation` so settle has deterministic inputs.
2) **Add settle hook to enforcer invoker**:
   - Extend `CapEnforcerInvoker` with `settle(module, CapSettleInput) -> CapSettleOutput`.
   - Implement in `PureCapEnforcer`.
3) **Invoke on receipt** (`EffectManager::settle_receipt`):
   - If `enforcer_module == sys/CapAllowAll@1`, skip and return empty usage.
   - Else if enforcer invoker present, call `settle` and use its `usage`.
   - Else fallback to built-in usage extraction for known effects (LLM, blob), or empty usage.
4) **Decision/journal**:
   - Record settle usage (and violation if present).
   - Pin the enforcer identity used for reserve/settle (module hash or manifest+name).
   - Decide policy for `violation` (error vs warning + ledger update).

This keeps cap logic centralized and makes it easy to add new cap types without leaking policy or budget logic into adapters.

Note: the enforcer module returns *requirements* (constraints + reserve estimate). The kernel MUST own ledger comparisons and mutations. Modules must not read or mutate ledger state; they only interpret cap semantics deterministically. If budget context is ever passed for estimation, name it `budget_hint` and state in the ABI that it is non-authoritative.

### Authorizer Pipeline (Kernel-Owned Ledger)
1) Canonicalize effect params (schema + secret normalization) into the exact CBOR bytes used for intent hashing/journaling.
2) Run cap enforcer module on those canonical values -> returns `{ constraints_ok, reserve_estimate, explain }`.
3) Kernel checks expiry + ledger budgets + writes reservation.
4) Kernel decides allow/deny and journals decision + reservation deltas.

The enforcer must see the same canonical input that is hashed as the intent identity, so authorization matches what is journaled.

Journal record for an authorization should include (at minimum):
- enforcer module identity (module hash, or manifest hash + module name resolved at that height)
- effect intent hash (derived from the canonical params)
- grant name (or grant hash)
- enforcer output (or a hash of it), including `constraints_ok` and `reserve_estimate`
- expiry check result
- ledger check result
- reservation delta (or settlement delta on receipt)

### Policy Ordering and Ledger Interaction
Policy should be evaluated **after** cap constraints and **before** committing reservations:

1) canonicalize params
2) cap enforcer → `{constraints_ok, reserve_estimate}`
3) policy → allow/deny/require_approval
4) if allow/require_approval: commit reservation + policy deltas
5) else: no ledger mutation

This preserves “cap before policy” semantics without a reserve→deny→refund cycle.

**RequireApproval** should hold the reservation once created. That prevents “approved but
now out of budget” outcomes and provides governance-grade backpressure.

**Policy decisions must be journaled** the same way cap decisions are, or approvals and
counters become non-replayable.

### Spec/Process Consistency (Avoid Hidden Kernel Magic)
If pure modules are part of the authority boundary, the AIR surface must reflect it:
- `defcap.enforcer` must be representable (already in spec).
- `defpolicy.engine` (or equivalent) must be representable if policy engines become modules.
- `defmodule.module_kind = "pure"` must be canonically expressible.

This keeps the system homoiconic: “data + modules,” not kernel-only behavior.

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
- HTTP enforcement can parse URLs in the enforcer module for now; long-term, move parsing into structured params or a deterministic normalizer.
- **Bounded vs unbounded dimensions:** For bounded dimensions (e.g., `tokens` reserved as `max_tokens`), enforce `actual <= reserved` at settle. For unbounded dimensions, reserve `0` and allow spend at settle.

---

## Budgets: Two-Phase Reserve -> Settle (Diamond Upgrade)

To avoid both oversubscription and over-counting:

1) **Reserve at enqueue**
   - Compute a conservative upper bound reservation.
   - Deny if insufficient budget.
   - Journal the reservation.

2) **Settle at receipt**
   - Compute actual usage from receipt (tokens/bytes/cents where available).
   - Refund unused reservation.
   - Journal the settlement.

Default settle rule for bounded dimensions:
- For dimensions where the reserve is intended to be an upper bound (e.g., `tokens` reserved as `max_tokens`), require `actual_usage <= reserved`. If violated, treat it as an adapter/receipt contract failure.
- For unbounded dimensions, reserve `0` and allow settle to add spent. This does not prevent oversubscription and should be used only when bounding is impossible.

Practical v1 reservations:
- `llm.generate`: reserve `max_tokens` (optionally a conservative prompt estimate); settle on receipt usage.
- `blob.put`: reserve known size (CAS metadata if available); settle on receipt size.
- `cost_cents`: reserve 0 unless bounded; settle from receipt cost if provided.

---

## Expiry Requires Deterministic "Now"

Expiry must not read wallclock in the kernel. See `roadmap/v0.5-caps-policy/p3-time.md` for the deterministic clock model.

Baseline rule:
- Use `logical_now_ns` (advanced only by trusted receipts) to evaluate `expiry_ns`.
- Do **not** reinterpret `expiry_ns` as journal height. If height-based expiry is required, treat it as a policy rule or introduce an explicit field (documented as a required spec change below).

---

## Make Budgets Open-Ended (Avoid a Closed-World Trap)
Budgets should be a `map<text,nat>` (or `map<text,dec128>` later), not a fixed struct. This allows adapters and enforcers to introduce new dimensions (`requests`, `gpu_ms`, `emails_sent`, `usd_micros`) without kernel changes, while still standardizing conventional names (`tokens`, `bytes`, `cents`).

Ledger modeling stays generic and opaque to the kernel:
- The kernel stores per-grant ledger state as `map<dimension, {limit,reserved,spent}>` (or equivalent), where `dimension` is just a string key.
- The kernel only performs arithmetic (compare/add/subtract) on these counters; it never interprets dimension names.
- Enforcers emit `reserve_estimate` and `actual_usage` as open-ended maps; the kernel applies deltas by key.
- Missing dimensions in a grant mean **unlimited** (no ledger check or reservation for that dimension).

Ledger invariant (per grant, per dimension):
- At enqueue: require `spent + reserved + reserve_estimate <= limit`.
- On settle: `reserved -= reserve_estimate`; `spent += actual_usage`.

---

## Param Normalization (Out of Scope for v0.5)

Two deterministic options:

1) **Structured URL schema**: add `sys/Url@1` and update HTTP params to carry structured fields (scheme/host/port/path). Authoring sugar can still accept strings and normalize at load/canonicalization time.
2) **Pure normalizer module**: let `defeffect` optionally reference a deterministic normalizer module that rewrites params into canonical form before hashing/enforcement/dispatch.

Parsing inside the enforcer is fine for v0.5. Normalization is tracked separately and is not part of this milestone.

---

## Minimal "Working Cap System" Requirements

1) [x] **Cap param enforcement**
   - Enforce cap constraints against effect params at enqueue.
   - Journal allow/deny decisions with structured reasons (journal is the only log).

2) [x] **Two-phase budget ledger**
   - Reservation at enqueue; settlement at receipt.

3) [x] **Expiry enforcement**
   - Use deterministic "now" and deny expired caps.

4) [x] **Audit trail**
   - Journal allow/deny decisions with reasons and budget deltas.
   - Include enforcer module identity and output (or a hash) for replay determinism.

---

## Proposed Minimal Use-Cases (Tests/Examples)

1) [x] **HTTP allowlist**
   - Cap params include `hosts` (and optional `methods`, `schemes`, `ports`, `path_prefixes`).
   - Allowed host -> ok, disallowed host -> denied (with reason).

2) [x] **LLM model allowlist + token budget**
   - Cap params include `allowed_models` and budget.
   - Disallowed model -> denied.
   - Budget reserved at enqueue, settled on receipt.

3) [x] **Blob constraints**
   - No cap constraints in v0.5 (cap exists to match effect kind only).

---

## FAQ (Current Questions)

### If we have enforcers, do we still need policies?
Yes. Caps answer **"can ever"** (delegated authority + constraints). Policies answer **"should now"** (governance gates, approvals, counters). Collapsing them pushes cap semantics into policy anyway, losing least-privilege reasoning and composability.

### Do identical effects collide on intent hash today?
Yes. The kernel computes `intent_hash = sha256(cbor(kind, params, cap, idempotency_key))`, and currently uses a zero idempotency key everywhere. If you emit the same effect (same kind/params/cap) twice, it shares an intent hash and cannot be safely in-flight concurrently. Until idempotency keys are exposed in AIR/ABI, callers must include a unique field in params (e.g., `request_id`) when they need distinct in-flight intents.

### Why not parse URLs only inside the enforcer?
You can. The reason to consider a separate normalizer or structured schema is that parsing then affects **authorization** but not **intent identity**. If normalization happens before hashing, the journaled intent matches the semantic URL, which improves explainability and determinism for downstream policy and caching.

### Is running an enforcer module on every effect too slow?
Likely acceptable: canonicalization, journaling, and external I/O dominate. For hot paths, keep enforcer logic small, return requirements rather than doing ledger checks, or use tiny “always-allow” enforcers for trivial caps.

### Are “pure modules” still pure if we pass ledger state?
Yes. Passing state explicitly still yields a referentially transparent function (same input → same output). “Pure” here means deterministic and side-effect-free, not stateless.

### What is the policy model in v0.5?
Policy stays data-only (`RulePolicy`) for v0.5; it is effectively a built-in policy engine. Later, policy can optionally be a pure module engine (or a built-in `RulePolicyEngine@1` module) without changing the kernel boundary.

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
5) Settle ABI wiring (enforcer settle invocation + ledger/journal)
6) Policy decision journaling + approval hold semantics
7) Per-intent reservation records + release path
8) Plan-level cap slots + grant hash
9) Cap-type interface stabilization + tests/fixtures

---

## Summary of Required Work

1) [x] Implement cap param enforcement at enqueue.
2) [x] Implement budget reservation + settlement with ledgered deltas.
3) [x] Implement deterministic expiry enforcement.
4) [x] Journal cap decisions with rationale.
5) [~] Wire settle ABI to enforcers (schemas exist; invocation + usage flow pending).
6) [~] Journal policy decisions + define approval hold behavior.
7) [~] Add per-intent reservation records + deterministic release path.
8) [~] Add plan-level cap slots + grant hash journaling.
9) [~] Add minimal use-case tests and replay checks (kernel unit tests + enforcer integration tests added; host-level replay coverage pending).

Once these exist, caps are a real security and budget control surface, not just wiring.

---

## Required Spec/Schema Updates (Status)

The following changes were required to make the design enforceable in AIR; status noted:

1) **defcap**: add `enforcer` module reference (pure module). (DONE)
2) **Built-in schemas**: add `sys/CapCheckInput@1`, `sys/CapCheckOutput@1`, `sys/CapSettleInput@1`, `sys/CapSettleOutput@1`. (DONE)
3) **Journal records**: define canonical cap decision + settle records that pin intent hash, enforcer identity, constraints result, reservation delta, usage deltas, and expiry/budget check outcomes. (PARTIAL: kernel record exists; spec update pending)
4) **Deterministic time inputs**: standardize `journal_height` + `logical_now_ns` in cap authorizer context (see `roadmap/v0.5-caps-policy/p3-time.md`). (PARTIAL: kernel uses `logical_now_ns`; spec update pending)
5) **Effect idempotency keys**: add optional `idempotency_key` to plan emit effects and reducer effects (AIR schema + WASM ABI), and thread it into `EffectIntent` hashing. (DONE)
6) **Plan-level cap slots**: add `plan_bindings` (or equivalent) to manifest and `cap_slot` to plan emit steps. (PENDING)
7) **Policy engine reference**: add `defpolicy.engine` (or equivalent) if policy is module-based. (PENDING)
8) **Grant hash**: standardize `grant_hash` in cap decision journals and explainers. (PENDING)
