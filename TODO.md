# TODO

When done with a todo, mark it as done, and a quick note what you achieved.

## Next Steps

- [x] Document effect-param canonicalization invariant in specs (spec/03-air.md, spec/02-architecture.md).
- [x] Add `normalize_effect_params(kind, params_cbor)` helper in `aos-effects` that decodes via the effect kind's param schema, canonicalizes using AIR rules, and returns canonical CBOR (reject on schema mismatch). Added `normalize.rs` + unit tests.
- [x] Wire the normalizer at the Effect Manager boundary for **all** sources (plan engine, reducer micro-effects, injected tooling) before intent hashing/CAS storage; drop pre-normalized bytes. EffectManager now canonicalizes post-secret handling before hashing.
- [x] Update secret walker to assume canonical `$tag/$value` only; sugar variants removed and injection re-encodes canonically (policy logic unchanged).
- [ ] Add golden tests: multiple sugar encodings of the same params must yield identical `intent_hash`/`params_ref`; reducer-emitted canonical params must round-trip unchanged. (partial: header-order unit test in `aos-effects`; plan LLM float-vs-string canonicalization + reducer timer canonicalization + hash stability integration tests in `crates/aos-testkit/tests/effect_params_normalization.rs`)
- [x] Update debug/inspection tooling (journal viewers, shadow reports) to render canonical params back to human-friendly JSON so “as-authored” visibility is preserved. Shadow summaries now include `params_json` for predicted effects.
