# TODO

When done with a todo, mark it as done, and a quick note what you achieved.

## Next Steps

- [x] Document effect-param canonicalization invariant in specs (spec/03-air.md, spec/02-architecture.md).
- [x] Add `normalize_effect_params(kind, params_cbor)` helper in `aos-effects` that decodes via the effect kind's param schema, canonicalizes using AIR rules, and returns canonical CBOR (reject on schema mismatch). Added `normalize.rs` + unit tests.
- [x] Wire the normalizer at the Effect Manager boundary for **all** sources (plan engine, reducer micro-effects, injected tooling) before intent hashing/CAS storage; drop pre-normalized bytes. EffectManager now canonicalizes post-secret handling before hashing.
- [x] Update secret walker to assume canonical `$tag/$value` only; sugar variants removed and injection re-encodes canonically (policy logic unchanged).
- [x] Add golden tests: multiple sugar encodings of the same params must yield identical `intent_hash`/`params_ref`; reducer-emitted canonical params must round-trip unchanged. (coverage in `aos-effects` + `crates/aos-testkit/tests/effect_params_normalization.rs`: header ordering, LLM float/string, HTTP sugar vs canonical, reducer timer hash stability, reducer micro-effect journal replay)
- [x] Update debug/inspection tooling (journal viewers, shadow reports) to render canonical params back to human-friendly JSON so “as-authored” visibility is preserved. Shadow summaries now include `params_json` for predicted effects.
- [ ] Restore dynamic emit_effect params support post-normalization (Expr JSON should remain Expr; allow null via ExprConst::Null). Add regression tests and fix examples (aggregator) accordingly. Also document migration flag for pre-normalization journals.

## Open Problem: dynamic emit_effect params broken after canonicalization changes

### Symptoms
- Plans that set `emit_effect.params` as an **expression** (e.g., `{"op":"get","args":[{"ref":"@plan.input"}, {"text":"url"}]}` inside a record) now fail normalization with “record missing field 'method'” or “expected scalar”.
- Examples `fetch-notify` and `aggregator` broke: HTTP effect params built from plan input are rejected; only fully literal params work.
- Test gap: no regression test covered “emit_effect params as Expr JSON” before the change.

### Root causes
1) **JSON → Literal by default**: `normalize_expr_or_value` converts `ExprOrValue::Json` into a `ValueLiteral` after canonicalization/validation. That path expects a fully literal record; any nested `op/get` turns into invalid JSON literal.
2) **Null handling**: we added `ExprConst::Null` but plans still use `null` for option fields. When JSON is treated as literal, `null` is fine; when treated as Expr, `null` must be encoded as `{"const":{"null":{}}}` to survive deserialization into `ExprConst::Null`.
3) **Schema enforcement timing**: effect params are now schema-validated after canonicalization. Expressions must stay as `Expr` so they can be evaluated at runtime; turning them into literals violates the schema (missing required fields) before evaluation.

### Desired behavior
- `emit_effect.params` may be **Expr** or **Literal**. If JSON parses as `Expr`, keep it as `Expr` (no literal canonicalization). Only literal JSON should be canonicalized/validated at load time.
- Option fields like `body_ref` should allow `ExprConst::Null` (via `{ "const": { "null": {} } }`) and literal `null`.
- Dynamic params must pass through to the plan evaluator, then normalize canonical CBOR at the Effect Manager boundary (already in place).

### Proposed fixes
1) **Expr-first JSON parsing (keeper)**  
   - Already added: attempt `serde_json::from_value<Expr>` before literal parsing. Keep this.
2) **Improve Expr JSON compatibility**  
   - Ensure plan literals support `{"const":{"null":{}}}` mapping to `ExprConst::Null`.
   - Document that option/null in Expr JSON should use this form; literal `null` remains valid when the whole params is a literal.
3) **Regression tests**
   - **Unit (air-types)**: `emit_effect.params` as JSON Expr containing `op:get`, headers map, and `body_ref` as `{"const":{"null":{}}}`; assert `normalize_plan_literals` leaves `params` as `Expr`.
   - **Integration (aos-testkit)**: build a tiny plan that reads url/method from input, emits http.request; assert it normalizes and enqueues intent (no schema errors).
   - **Example smoke**: rerun `fetch-notify` and `aggregator` after fixes.
4) **Optionally** add a helper in authoring docs: for Expr JSON, wrap null as `{ "const": { "null": {} } }`.

### Work items
- [ ] In `plan_literals.rs` test module, add regression test for JSON Expr params (Expr kept). Fix test failure by accepting `ExprConst::Null` and not requiring literal fields when params are Expr.
- [ ] Adjust `normalize_expr_or_value`: when JSON parses as Expr, **skip literal path entirely** (no validation/canonicalization). Ensure no subsequent code re-canonicalizes it as literal.
- [ ] Confirm `ExprConst::Null` is deserializable from `{ "const": { "null": {} } }` (serde derives should handle via `ExprConst::Null { null: {} }`).
- [ ] Update example plans (04-aggregator, 03-fetch-notify) to use Expr params with `body_ref` as `{"const":{"null":{}}}` if needed; keep dynamic method/url sourced from input.
- [ ] Add integration test in `aos-examples` or `aos-testkit` that loads a plan with dynamic params and runs through EffectManager without schema errors.

### Out-of-scope / not needed
- Migration flag: not required (no pre-normalization journals; pre-alpha).
