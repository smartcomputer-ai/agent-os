# p1-pure-modules: Deterministic Pure Modules (Work + Rationale)

**Complete**

Status: complete (core pure module support shipped; downstream cap/policy/plan usage tracked in their own tasks).

## TL;DR
AgentOS already anticipates "pure components" (deterministic WASM functions) in the architecture/spec, but v1 only ships reducers. We should pull pure modules forward in v0.5 as a first-class module kind so that cap enforcers, policy engines, and plan compute helpers are all **data + modules**, not kernel code.

---

## Why This Matters Now

The current trajectory makes kernel behavior grow with every new cap type or policy feature. Pure modules are the escape hatch:

- **Caps/policies** become pinned, deterministic authorizers (modules), not kernel code.
- **Plans** gain an escape valve for heavy deterministic compute without bloating the plan expression language.
- **Adapters** can ship cap/policy logic as pure modules alongside the effect implementation.

This matches the architecture direction in `spec/02-architecture.md` ("pure components") and the AIR note in `spec/03-air.md` that `module_kind` may add `"pure"`.

---

## Spec Alignment (Current State)

- `spec/02-architecture.md` already names "pure components" as deterministic WASM modules.
- `spec/03-air.md` explicitly calls out that `module_kind` may add `"pure"` in a future version.
- Current loader/runtime only supports reducers.

This task pulls that future forward in a minimal, v0.5-compatible way.

---

## Proposed Semantics

### Module Kind

Extend `defmodule` with:

- `module_kind: "pure"`
- ABI: `run(input_bytes) -> output_bytes` (formal envelope, CBOR in/out)
- Inputs/outputs are canonical CBOR, pinned to a schema reference (like reducers).

### Determinism Profile

Same deterministic profile as reducers:

- no wallclock, no threads, no nondeterministic hostcalls
- stable float rules
- explicit hashing/serialization helpers only

### Purity Contract

Pure modules:

- **MUST NOT** emit effects
- **MUST NOT** mutate kernel state
- May accept state snapshots **as explicit input** (still pure)
- Return data + explanations only

---

## Where Pure Modules Fit (vs Reducers)

Use **reducers** for state machines:

- mutate durable state
- emit domain events / micro-effects

Use **pure modules** for deterministic compute:

- cap/policy evaluation
- param normalization and validation
- plan-level data transforms
- checksums, canonical projections, derived values

If the computation has no stateful side effects and can be expressed as a pure function over explicit inputs, it should be a pure module, not a reducer.

---

## Integration Points (v0.5 Targets)

### 1) Cap Enforcers

Add `defcap.enforcer.module`, pointing to a pure module.
Kernel calls it during authorization; module returns constraints + reserve estimates.
Kernel owns ledger checks and mutations.

### 2) Policy Engines

Add optional `defpolicy.engine.module`.
If present, kernel calls the module; otherwise use built-in RulePolicy.
Module returns decision + counter deltas; kernel applies deltas and journals.

### 3) Plan Compute Helpers (Optional for v0.5)

Plans are intentionally small and pure; adding "heavy compute" to plan expressions is not desirable.
Instead, allow plan steps to invoke a pure module for deterministic transformations:

- normalize inputs (e.g., URL parsing)
- compute derived values for guards
- validate complex payloads before emitting effects

This can be a dedicated plan op (e.g., `call_module`) or a pure "compute" step that binds its output into plan scope.

---

## Work Items

### Schema + Validation

1) Update `spec/schemas/defmodule.schema.json`:
   - allow `module_kind: "pure"`.
2) Add optional input/output schema refs for pure modules (if not already implied).
3) Update AIR validator to:
   - accept pure modules
   - verify schema refs exist and are compatible

### Loader + Runtime

1) Extend module registry to load and cache pure modules.
2) Add a deterministic runner:
   - `run(input_bytes) -> output_bytes`
   - same Wasmtime profile as reducers
3) Add a shared CBOR envelope for pure calls (or reuse existing typed CBOR decode path).

### SDK Support

1) Expand `aos-wasm-sdk` with helpers/macros for pure modules, mirroring reducer ergonomics.
2) Provide CBOR (de)serialization helpers for pure input/output schemas.
3) Split SDK modules into `reducers` and `pure` for clarity.

### Kernel Call Sites

1) Cap enforcer call in `EffectManager::enqueue_effect`.
2) Policy engine call in the policy gate.
3) Journal structured decisions and deltas.

### Plan Integration (Optional in v0.5)

1) Plan op to invoke a pure module.
2) Bind outputs into plan scope for guards/effect params.
3) Deterministic error handling (fail step vs. deny).

### Tests

1) Pure module runner unit tests (determinism, CBOR IO).
2) Cap enforcer integration test (allow/deny + budget reservation).
3) Policy engine integration test (allow/deny + counter deltas).
4) Plan call (if implemented) with deterministic transform.

---

## Open Questions / Decisions

1) **Export name**: `run` (mirrors architecture docs and keeps reducers on `step`).
2) **Plan op shape**: a new `call_module` step vs. expression function?
3) **Hostcalls**: which helper intrinsics are allowed for pure modules?
4) **Caching**: should pure module outputs be memoized by input hash at runtime?

---

## Outcome (Why This Is Worth It)

Pure modules let us keep the kernel small and deterministic while making caps, policy, and plan compute open-ended and data-driven. This is the same "module + manifest" model as reducers and plans, and it is the cleanest path to adapter extensibility without a kernel treadmill.
