# p2-caps: Capability System (Constraints-Only Direction)

## TL;DR
Caps are enforced in the kernel at enqueue time (grant exists, cap type matches effect kind, reducer slot binding exists). Cap params and expiry are enforced and decisions are journaled. Cap semantics live in pure enforcer modules (builtin allow-all fallback). Budgets and the ledger/settlement pipeline are removed from v0.5; see `roadmap/vX-future/p4-budgets.md` for the preserved budget design.

Remaining work is now focused on: removing budget fields and ledger/settle code, tightening cap decision journaling, spec updates, and tests/host-level replay coverage.

---

## Direction Change (v0.5): Caps Without Budgets
We are explicitly removing budgets from v0.5. Caps become constraints-only authority handles with optional expiry, and enforcers become pure constraint checkers. The kernel no longer owns a cap ledger or settlement pipeline.

**Required changes to make this real:**
1) Remove `budget` from `CapGrant` in AIR schema + Rust types.
2) Remove the cap ledger, reservations, and settlement logic from the kernel.
3) Simplify the cap enforcer ABI: drop `reserve_estimate` and remove `CapSettleInput/Output`.
4) Remove reserve/usage fields from cap decision journals and snapshots.
5) Update tests/examples/docs to remove budget language.
6) Preserve budget design in `roadmap/vX-future/p4-budgets.md`.

---

## Diamond Invariants (Design Spine)

1) **One authoritative authorizer, one deterministic transaction**
   - Kernel authorization must be the single decision point:
     - canonicalize params -> cap check -> policy check -> journal decisions -> enqueue or deny
   - Any mutation (approvals, counters, governance records) must be part of that same deterministic, journaled transaction.
   - The enforcer never makes the final allow/deny decision. It only returns semantic constraint results; the kernel checks expiry and records the final decision.

2) **Caps and policy stay orthogonal in shape**
   - Caps answer: "can ever" by returning constraints.
   - Policies answer: "should now" and return a structured decision.
   - The kernel composes them in fixed order: cap -> policy.

3) **Every decision is explainable and replayable**
   - Allowed/denied outcomes must be derivable from the journal alone.
   - Cap denials should record *what constraint failed*.
   - Journal entries must pin determinism: enforcer module identity, intent/grant identity, and enforcer output (or hash).

---

## What Caps Are For (Conceptual Model)
Caps are **static, typed grants** that authorize which effect kinds a world may emit, plus **parameters** that constrain how those effects may be used (e.g., allowed HTTP hosts, LLM models, max body size). The invariant is:

- If a plan/reducer can emit an effect, it must present a **cap grant** whose **cap type** matches that effect kind and whose **params** authorize that specific use.

Caps must be deterministic, auditable, and enforced **before** enqueue. Budgets are explicitly out of scope for v0.5.

### Cap Grants vs DefCaps (Clarification)
**Defcaps** define the *type* of capability: cap type, param schema, and enforcer module.
**Cap grants** are concrete, named instances of a defcap with fixed params and expiry.

Key rules:
- Plans and reducers reference **cap grants** (grant names), not defcap names.
- Enforcers do **not** mint grants; they evaluate a request against an existing grant.
- A grant's `cap` field points to the defcap that defines its schema + enforcer.

Put differently: defcap = template, grant = instantiated authority, enforcer = checker.

### Call-Trace: Grant Params -> Enforcer Check (Fetch-Notify)
This is the concrete data path for the `example.com` allowlist:

```
manifest.defaults.cap_grants["cap_http_fetch"].params
  -> resolve_grant(): validate + canonicalize params to CBOR
  -> CapabilityGrant.params_cbor
  -> enqueue_effect(): cap_constraints(...)
  -> CapCheckInput.cap_params = params_cbor
  -> sys/CapEnforceHttpOut@1 decodes cap_params.hosts
  -> compare URL host from effect_params.url against hosts allowlist
```

So the enforcer does not "know" about `example.com` magically; it is fed the grant
params every time, in canonical CBOR form, and compares them to the effect's URL.

### Cap Reference Validation (How It Should Work)
Cap references must be validated against **grants**, not defcaps. Validation should enforce:

1) **Grant existence**
   - Every `emit.cap` in a plan, every `required_caps`, every module `cap_slots` binding,
     and every `secret.allowed_caps` entry must reference a **grant name** in
     `manifest.defaults.cap_grants`.

2) **Grant -> defcap correctness**
   - Each grant's `cap` must reference a defcap in `manifest.caps`.
   - Grant params must validate against the defcap schema (already enforced in kernel load).

3) **Effect kind <-> cap type match (via the grant)**
   - For each `emit_effect` step, look up its grant, then the grant's defcap,
     and verify `defcap.cap_type` matches the effect kind's required cap type.
   - This check should use grants as the primary lookup key; defcaps are only templates.

4) **Uniqueness + consistency**
   - Grant names must be unique.
   - A single grant may be referenced in multiple plans/modules, but all uses must be
     compatible with its defcap cap type.

This makes the authoring model consistent with runtime: **grants are the capability boundary**, and defcaps are the templates.

### Cap Grants: Practical Improvements (Ergonomics + Audit)
Two small changes make grant usage more evolvable and auditable:

1) **Plan-level cap slots (mirror reducer slots)**
   - Today: reducers use `cap_slot` -> manifest binds slots to grants; plans reference grants directly.
   - Proposed: allow plans to declare `cap_slot: "http_default"` and add
     `manifest.plan_bindings[plan_name].slots.http_default = "grant_name"`.
   - Benefit: governance can swap grants without touching plan defs; same plan can be wired
     differently across worlds.

2) **Stable grant hash**
   - Keep human-friendly names, but compute:
     `grant_hash = sha256(cbor({defcap_ref, params_cbor, expiry}))`.
   - Journal the hash alongside decisions. This makes "same name, changed meaning"
     detectable and gives adapters/logs a stable identifier without exposing params.

---

## Design Smell (Current Trajectory)
Previously, cap semantics lived in the kernel. A `defcap` only defined param **shape**, while the **meaning** of those params was hardcoded (hosts mean URL host allowlists, max_tokens mean ceilings, etc.). That made caps a closed-world enum disguised as open strings, and would block dynamic adapter/cap addition.

This is now addressed by pure enforcer modules, with builtin allow-all fallback for simple caps.

The diamond invariant already points to the fix: the **kernel is the authoritative transaction boundary**, but the logic executed inside that boundary does not have to be hardcoded. It only needs to be deterministic, pinned, and journaled.

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
- Reducer micro-effects include optional `cap_slot` (default: "default").
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
- This happens before any policy decision, and before cap enforcement.

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

### 8) Expiry is enforced
- `expiry_ns` checked against deterministic `logical_now_ns`.

Relevant code:
- `crates/aos-kernel/src/effects.rs`

---

## Gap Status (Constraints-Only)

1) **Remove budgeted-cap machinery**
   - Remove the ledger/reservation/settle path in the kernel.
   - Remove budget fields from AIR schemas and Rust types.
   - Remove reserve/usage from cap decision records and snapshots.

2) **Simplify cap enforcer ABI**
   - Drop `reserve_estimate` and remove the `CapSettle*` schemas.
   - Keep a constraints-only `CapCheckOutput`.

3) **Policy decision journaling**
   - Journal allow/deny decisions for replay/audit.

4) **Host-level replay coverage**
   - Expand integration tests that replay cap decisions across journal/snapshot boundaries.

5) **Plan-level cap slots**
   - Allow plans to use slots that are bound to grants via `manifest.plan_bindings`.

6) **Stable grant hash in journals**
   - Compute + record a `grant_hash` alongside cap decisions.

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
The schema already supports deterministic pure modules and the kernel can invoke them. The ABI remains:

- `module_kind: "pure"`
- `run(input_bytes) -> output_bytes` using canonical CBOR in/out (schema-pinned).

---

## Cap Enforcer ABI (Constraints-Only)
Budgets are removed; enforcers only evaluate constraints.

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
  deny?: { code, message }
}
```

The budget-aware ABI (reserve/settle) is preserved in `roadmap/vX-future/p4-budgets.md`.

---

## Authorizer Pipeline (No Ledger)
1) Canonicalize effect params (schema + secret normalization).
2) Run cap enforcer module on those canonical values -> `{ constraints_ok, deny? }`.
3) Kernel checks expiry and policy.
4) Kernel journals decision + enforcer identity.
5) Kernel enqueues or denies the effect.

The enforcer must see the same canonical input that is hashed as the intent identity, so authorization matches what is journaled.

Journal record for an authorization should include (at minimum):
- enforcer module identity (module hash, or manifest hash + module name resolved at that height)
- effect intent hash (derived from the canonical params)
- grant name (or grant hash)
- enforcer output (or a hash of it), including `constraints_ok`
- expiry check result

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
- `max_tokens` is a constraint, not a budget. No usage accounting in v0.5.
- HTTP enforcement can parse URLs in the enforcer module for now; long-term, move parsing into structured params or a deterministic normalizer.

---

## Expiry Requires Deterministic "Now"
Expiry must not read wallclock in the kernel. See `roadmap/v0.5-caps-policy/p3-time.md` for the deterministic clock model.

Baseline rule:
- Use `logical_now_ns` (advanced only by trusted receipts) to evaluate `expiry_ns`.
- Do **not** reinterpret `expiry_ns` as journal height. If height-based expiry is required, treat it as a policy rule or introduce an explicit field.

---

## Minimal "Working Cap System" Requirements

1) [x] **Cap param enforcement**
   - Enforce cap constraints against effect params at enqueue.
   - Journal allow/deny decisions with structured reasons (journal is the only log).

2) [x] **Expiry enforcement**
   - Use deterministic "now" and deny expired caps.

3) [x] **Audit trail**
   - Journal allow/deny decisions with reasons.
   - Include enforcer module identity and output (or a hash) for replay determinism.

---

## Proposed Minimal Use-Cases (Tests/Examples)

1) [x] **HTTP allowlist**
   - Cap params include `hosts` (and optional `methods`, `schemes`, `ports`, `path_prefixes`).
   - Allowed host -> ok, disallowed host -> denied (with reason).

2) [x] **LLM model allowlist + max_tokens constraint**
   - Cap params include `allowed_models` and optional `max_tokens` ceiling.
   - Disallowed model -> denied; max_tokens above cap -> denied.

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
Likely acceptable: canonicalization, journaling, and external I/O dominate. For hot paths, keep enforcer logic small, or use tiny "always-allow" enforcers for trivial caps.

### Where are budgets?
Budgets are explicitly out of scope for v0.5 and captured in `roadmap/vX-future/p4-budgets.md`.

---

## Governance Relationship

- Caps are manifest-level objects; governance patches add/modify defcaps and grants.
- Shadow runs load the patched manifest, so cap wiring is exercised during shadow.
- Governance-level caps (who can propose what) are separate and still TODO.

---

## Build Order (Constraints-Only)

1) Cap param enforcement
2) Journaled cap decisions (allow/deny + reasons)
3) Expiry enforcement with deterministic "now"
4) Remove budget machinery (ledger, settle, schemas, tests)
5) Policy decision journaling + approval hold semantics
6) Plan-level cap slots + grant hash
7) Cap-type interface stabilization + tests/fixtures

---

## Summary of Required Work

1) [~] Remove budget fields, ledger, reservations, and settle logic (kernel + schemas + journals + snapshots).
2) [~] Simplify cap enforcer ABI to constraints-only output.
3) [~] Journal policy decisions and define approval hold behavior.
4) [~] Add plan-level cap slots + grant hash journaling.
5) [~] Add minimal use-case tests and replay checks (host-level replay coverage pending).

Once these exist, caps are a real security surface with deterministic constraints and auditability.

---

## Required Spec/Schema Updates (Status)

1) **defcap**: add `enforcer` module reference (pure module). (DONE)
2) **Built-in schemas**: update `sys/CapCheckOutput@1` to remove reserve fields; remove `sys/CapSettleInput@1` and `sys/CapSettleOutput@1`. (PENDING)
3) **Cap grants**: remove `budget` from `CapGrant` schema and Rust types. (PENDING)
4) **Journal records**: define canonical cap decision records that pin intent hash, enforcer identity, constraints result, and expiry check outcomes (no reserve/usage). (PARTIAL)
5) **Deterministic time inputs**: standardize `logical_now_ns` in cap authorizer context (see `roadmap/v0.5-caps-policy/p3-time.md`). (PARTIAL)
6) **Effect idempotency keys**: add optional `idempotency_key` to plan emit effects and reducer effects (AIR schema + WASM ABI), and thread it into `EffectIntent` hashing. (DONE)
7) **Plan-level cap slots**: add `plan_bindings` (or equivalent) to manifest and `cap_slot` to plan emit steps. (PENDING)
8) **Policy engine reference**: add `defpolicy.engine` (or equivalent) if policy is module-based. (PENDING)
9) **Grant hash**: standardize `grant_hash` in cap decision journals and explainers. (PENDING)

