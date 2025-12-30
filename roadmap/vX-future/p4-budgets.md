# p4-budgets: Capability Budgets (Future)

## TL;DR
Budgets are deferred from v0.5 caps. This doc captures the budgeted-cap design for a future milestone: a kernel-owned ledger with two-phase reserve -> settle, per-intent reservations, and deterministic journaling. Enforcers return reserve and usage hints; the kernel applies ledger mutations.

---

## Scope
This is future-facing and not wired in the kernel today. See `roadmap/v0.5-caps-policy/p2-caps.md` for the constraints-only cap system.

---

## Budget Model (Per Grant)
Budgets live on cap grants as a map of dimension -> limit. The kernel treats dimensions as opaque strings and only does arithmetic.

- `budget: map<text,nat>` on each grant
- Ledger entry per grant, per dimension: `{ limit, reserved, spent }`
- Reservations are created per intent; settlement uses the reservation that created the intent

This keeps the kernel generic and enables new cost dimensions (tokens, bytes, cents, gpu_ms) without kernel changes.

---

## Enforcer ABI (Budget-Aware)
Enforcers remain deterministic pure modules. With budgets enabled, they return reservation estimates at enqueue and usage at receipt.

### Check (enqueue)

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

CapCheckOutput = {
  constraints_ok: bool,
  deny?: { code, message },
  reserve_estimate: map<text,nat>
}
```

### Settle (receipt)

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
  violation?: { code, message }
}
```

### Tagged union wrapper
Pure modules expose a single `run(input_bytes) -> output_bytes`. To keep one ABI surface:

```cbor
CapEnforcerInput = variant { Check: CapCheckInput, Settle: CapSettleInput }
CapEnforcerOutput = variant { Check: CapCheckOutput, Settle: CapSettleOutput }
```

---

## Ledger Invariants (Correctness Over Time)
Two-phase accounting only stays correct if reservations are tracked per intent, not just aggregate counters. Model reservations as first-class records keyed by `intent_hash`:

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
- `spent_total[dim]` is monotone-increasing based on settle usage

This makes settlement idempotent: duplicate receipts can be ignored when status != "reserved".

---

## Pin Enforcer Identity for Settlement
When a reservation is created, record the enforcer identity used for the check (module hash or `{manifest_hash, module_name}`). On settle, use the same identity to interpret receipts. This prevents semantic drift across upgrades while intents are in flight.

---

## Intent Outcomes (Must Be Explicit)
Every reserved intent should end in exactly one of:

1) Receipt arrives -> settle
2) Explicit release -> reservation released (cancel/expiry/governance)
3) No receipt -> reservation remains reserved (operationally visible)

This implies a deterministic release path and introspection tooling to see outstanding reservations by grant + dimension.

---

## Settlement Semantics
Receipts should always settle:

- If receipt lacks usage for a dimension, treat usage as 0 for that dimension.
- Always release the reservation, even on error/timeout.
- Policy may still count attempts separately; cap budgets should charge actual usage.

For bounded dimensions, enforce `actual <= reserved`. For unbounded dimensions, reserve 0 and allow spend at settle (only when no upper bound is possible).

---

## Wiring Plan (Kernel)
1) Capture enqueue context: store `effect_params` (canonical CBOR), `cap_params`, and `origin` in `CapReservation` so settle has deterministic inputs.
2) Add settle hook to enforcer invoker (`settle(module, CapSettleInput)`).
3) Invoke on receipt and apply usage deltas.
4) Journal settle usage and any violations.

The kernel owns ledger comparisons and mutations. Enforcers interpret cap semantics and return requirements; they do not read or mutate the ledger.

---

## Authorizer Pipeline (With Budgets)
1) Canonicalize effect params (schema + secret normalization).
2) Run cap enforcer -> `{ constraints_ok, reserve_estimate }`.
3) Kernel checks expiry + ledger budgets + writes reservation.
4) Kernel evaluates policy.
5) Journal decision and enqueue or deny.

The enforcer must see the same canonical input that is hashed into the intent identity.

---

## Policy Ordering and Ledger Interaction
Policy should be evaluated after cap constraints and before committing reservations:

1) canonicalize params
2) cap enforcer -> `{constraints_ok, reserve_estimate}`
3) policy -> allow/deny/require_approval
4) if allow/require_approval: commit reservation + policy deltas
5) else: no ledger mutation

Require-approval holds the reservation. Policy decisions must be journaled for replay/audit.

---

## Open-Ended Budget Dimensions
Budgets should be a `map<text,nat>`, not a fixed struct. This avoids a closed-world kernel:

- The kernel stores per-grant ledger state as `map<dimension, {limit,reserved,spent}>`.
- The kernel only performs arithmetic; it does not interpret dimension names.
- Missing dimensions in a grant mean unlimited (no ledger check or reservation).

---

## Bounded vs Unbounded Dimensions
- Bounded dimensions (tokens, bytes): reserve an upper bound at enqueue, require `usage <= reserve` at settle.
- Unbounded dimensions: reserve 0 and allow spend at settle. Use only when no bounding is possible.

---

## Budgeted Use Cases
- `llm.generate`: reserve `max_tokens`, settle on receipt usage.
- `blob.put`: reserve known size, settle on receipt size.
- `cost_cents`: reserve 0 unless bounded, settle from receipt cost.

---

## Stable Grant Hash (Budgeted)
Keep human-friendly names, but compute:

```
grant_hash = sha256(cbor({defcap_ref, params_cbor, budget, expiry}))
```

Journal the hash alongside decisions to make "same name, changed meaning" detectable.

---

## Spec/Schema Notes (When Budgets Return)
- `CapGrant` includes optional `budget` (map<text,nat>).
- Built-in schemas include `sys/CapCheck*` and `sys/CapSettle*` with reserve/usage fields.
- Journal records must include reservation and settlement deltas.

