# TODO

When done with a todo, mark it as done, and a quick note what you achieved.

## Next Steps

- [x] Document effect-param canonicalization invariant in specs (spec/03-air.md, spec/02-architecture.md).
- [ ] Add `normalize_effect_params(kind, params_cbor)` helper in `aos-effects` that decodes via the effect kind's param schema, canonicalizes using AIR rules, and returns canonical CBOR (reject on schema mismatch).
- [ ] Wire the normalizer at the Effect Manager boundary for **all** sources (plan engine, reducer micro-effects, injected tooling) before intent hashing/CAS storage; drop pre-normalized bytes.
- [ ] Update policy/secret walkers and adapter entrypoints to assume canonical `$tag/$value` shapes only (remove sugar fallbacks where possible).
- [ ] Add golden tests: multiple sugar encodings of the same params must yield identical `intent_hash`/`params_ref`; reducer-emitted canonical params must round-trip unchanged.
- [ ] Update debug/inspection tooling (journal viewers, shadow reports) to render canonical params back to human-friendly JSON so “as-authored” visibility is preserved.
- [ ] Note migration flag/feature gate if we ever need replay of pre-normalization journals; otherwise keep current “pre-alpha, no worlds” stance explicit in release notes.
