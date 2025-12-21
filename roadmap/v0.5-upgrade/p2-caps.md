# p2-caps: Capability System (Current State + Work Remaining)

## TL;DR
Capabilities exist and are enforced at enqueue time in the kernel (cap grant exists, cap type matches effect kind, reducer slot binding exists). However, cap grant **params**, **budgets**, and **expiry** are not enforced, and adapters do not receive or validate cap data. The system is structurally wired but not yet a complete security/budgeting model.

---

## What Caps Are For (Conceptual Model)
Caps are **static, typed grants** that authorize _which effects_ an agent world may emit, plus **parameters** that constrain how those effects may be used (e.g., allowed HTTP hosts, LLM models, max body size). The core invariant is:

- If a plan/reducer can emit an effect, it must present a **cap grant** whose **cap type** matches that effect kind, and whose **params** constrain the allowed use of that effect.

Caps should be deterministic, auditable, and enforced **before** an effect is enqueued.

---

## Current Implementation (What Works Today)

### 1) Cap grants are resolved and schema-validated at manifest load
- The kernel loads `manifest.defaults.cap_grants` and resolves each grant against its `defcap`.
- Grant params are validated against the defcap schema and encoded to canonical CBOR.
- Each grant is associated with a **cap type** (from the `defcap`).

Relevant code:
- `crates/aos-kernel/src/capability.rs` (resolve_grant, schema expansion/validation)

### 2) Cap type must match effect kind at enqueue
- When any effect is enqueued (plan or reducer), the kernel resolves the **cap grant** by name and verifies that its **cap type** matches the effect kind’s expected cap type (from the effect catalog).
- This blocks miswired cap/effect pairs.

Relevant code:
- `crates/aos-kernel/src/effects.rs` (enqueue_effect)
- `crates/aos-kernel/src/capability.rs` (expected_cap_type)

### 3) Plans must declare required caps
- `plan.required_caps` are enforced at manifest load. If a required cap grant is missing, the manifest is rejected.

Relevant code:
- `crates/aos-kernel/src/world.rs` (ensure_plan_capabilities)

### 4) Reducer cap slots are wired
- Reducers emit micro-effects with an optional `cap_slot` (default: `"default"`).
- The kernel maps that slot to a cap grant name using `manifest.module_bindings[reducer].slots`.
- Missing binding is an error.

Relevant code:
- `crates/aos-wasm-abi/src/lib.rs` (ReducerEffect.cap_slot)
- `crates/aos-kernel/src/world.rs` (cap_slot resolution and binding lookup)

### 5) Effect intents carry only `cap_name`
- `EffectIntent` includes `cap_name` but not the grant params, cap type, or any token.
- Adapters receive only the intent (kind, params, cap_name, hash).

Relevant code:
- `crates/aos-effects/src/intent.rs`
- adapters in `crates/aos-host/src/adapters/*`

---

## What Is Not Implemented (Gaps)

1) **Cap params are not enforced against effect params**
   - Caps define param constraints (e.g., allowed hosts/models), but no runtime checks compare cap params to effect params.
   - This is the largest missing “teeth” in the system.

2) **Budgets are not enforced**
   - Cap grants allow `budget` fields (tokens/bytes/cents), but no counter is decremented and no checks occur.

3) **Expiry is not enforced**
   - Cap grants can have `expiry_ns`, but it is ignored at runtime.

4) **Adapters do not validate cap grants**
   - Adapters never see cap params, so they cannot enforce host/model/limit restrictions.
   - This is intentional today, but means caps have no effect on actual I/O behavior.

5) **No policy/budget ledger integration**
   - There is no ledgered cap usage, no journal record for cap consumption, and no replayable budget state.

---

## Where Enforcement Should Live (Design Clarification)

**Primary enforcement should be in the kernel before enqueue**, because:
- It is deterministic and replayable.
- It is auditable (can be journaled).
- Adapters may be non-deterministic or environment-specific.

Adapters may still do _safety checks_ (e.g., HTTP timeouts, body size caps for local resources), but **cap compliance** must be enforced in the kernel.

If we want adapters to enforce caps, we must either:
- Embed cap params into the intent (expands intent hash / audit surface), or
- Provide a deterministic lookup from intent → cap grant params (store lookup), which the adapter can verify but not mutate.

Given determinism goals, kernel-side enforcement is the safest default.

---

## Minimal “Working Cap System” Requirements

These are the smallest steps needed for caps to be meaningful:

1) **Cap param enforcement**
   - Define a per-cap-type validator that compares cap params with effect params.
   - Example: `http.out` cap params `{ hosts: ["api.example.com"] }` must be enforced against `http.request.url`.
   - Enforcement happens inside `EffectManager::enqueue_effect` (or a new `CapPolicy` layer).

2) **Budget accounting**
   - Add a deterministic ledger for cap budgets (tokens/bytes/cents).
   - Decide when to decrement:
     - Option A: at enqueue time (predictive) — simple but may over-count on failed effects.
     - Option B: at receipt time (actual) — more accurate but needs receipt metadata (cost) to be reliable.
   - Enforce “insufficient budget” as a hard deny at enqueue.

3) **Expiry enforcement**
   - Use a deterministic time source (e.g., journal sequence or deterministic clock) to compare against `expiry_ns`.
   - Deny use of expired caps at enqueue.

4) **Audit trail**
   - Record cap decisions (and budget deltas) in the journal to keep replay deterministic.

---

## Proposed Concrete Use-Cases (Minimal Set)

Use cases should be explicit and testable:

1) **HTTP allowlist**
   - Cap params include `allowed_hosts`.
   - Plan emits `http.request` to an allowed host → allowed.
   - Plan emits to a different host → denied.

2) **LLM model allowlist + token budget**
   - Cap params include `allowed_models` and `token_budget`.
   - Effect with model not in list → denied.
   - Token budget decremented per receipt or predicted usage.

3) **Blob size limit**
   - Cap params include `max_bytes` for blob put.
   - Blob size exceeds → denied.

---

## Tests Needed (Suggested)

- Unit: `cap params vs effect params` validator per cap type.
- Integration: plan emit denied if cap params disallow host/model.
- Integration: cap budget decrement + deny on exhaustion.
- Replay: budget state is deterministic across replay.

---

## Governance Relationship

- Caps are manifest-level objects; **governance patches** add/modify defcaps and grants.
- Shadow runs already load the patched manifest, so cap wiring is exercised in shadow.
- However, there is no governance cap/policy to restrict which patches are allowed; this is a separate governance-level capability TODO.

---

## Summary of Required Work

1) Implement **cap param enforcement** at enqueue time.
2) Implement **budget ledger + decrement logic**.
3) Implement **expiry enforcement**.
4) Add **journal records** for cap decisions or budget deltas.
5) Add **tests and fixtures** for the minimal use cases.

Once the above is done, caps will have real “teeth” beyond structural validation.
