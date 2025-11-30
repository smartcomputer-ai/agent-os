# Polish 2: Close the Remaining Spec/Runtime Gaps

Status legend: ✅ already aligned | 🟡 partially | 🔴 not yet

this is pre, pre, pre alpha software. there is not a single instance of this OS besides what you see here in my repo. We have complete leeway in refacoring, this is why we're doing this cleanup/polish.

---

## 1) Require explicit `air_version` (stop “assume latest”)
- ✅ **Problem**: `air_version` was optional and defaulted to current major (spec/03-air.md lines ~110–129; spec/schemas/manifest.schema.json; crates/aos-store/src/manifest.rs:103-114). Future majors would silently upgrade old manifests.
- ✅ **Fix** (done):
  - `air_version` is now required and enumerated to `"1"` in `spec/schemas/manifest.schema.json`.
  - Prose updated in `spec/03-air.md` §4 to make the field required and remove the “assume latest” behavior.
  - Loader now errors when `air_version` is missing or unsupported (`crates/aos-store/src/manifest.rs` + new `StoreError::MissingAirVersion`).

## 2) Remove built-in auto-inclusion magic
- ✅ **Problem**: Runtime/tooling auto-attached built-in schemas/effects (spec/03-air.md; crates/aos-store/src/manifest.rs; crates/aos-kernel/src/manifest.rs), so manifest wasn’t the single source of truth.
- ✅ **Fix** (done):
  - Removed auto-attach in store/kernel loaders; manifests must list all built-in schemas/effects.
  - Store loader validates presence and fills canonical hashes for built-ins; missing ones raise `StoreError::MissingBuiltin`.
  - Spec updated to require explicit listing (no more “auto-include” prose).
  - Example manifests now include built-in schema/effect refs; authoring loader normalizes `effects` hashes too.

## 3) Simplify policy match surface (drop host/method)
- ✅ **Problem**: `defpolicy.Match` still contained HTTP-specific `host` and `method`, overlapping with CapGrant constraints.
- ✅ **Fix** (done):
  - Removed `host`/`method` from `spec/schemas/defpolicy.schema.json` and the spec bullets/example policy in `spec/03-air.md`.
  - Simplified policy model/runtime (aos-air-types, aos-kernel) to drop these fields; tests updated accordingly.

## 4) Validate `await_event` correlation at authoring time
- ✅ **Problem**: Runtime rejected missing `where` when `correlate_by` is set, but validation didn’t enforce it.
- ✅ **Fix** (done):
  - Store-level validation now enforces: if a plan is started via a trigger with `correlate_by`, every `await_event` must have a `where` predicate and it must reference the correlation key or `@var:correlation_id` (`crates/aos-store/src/manifest.rs` + new errors).
  - Spec text to update next (see #5) — runtime + validation behavior aligned.

## 5) Make micro-effect rule point to `origin_scope`
- 🟡 **Problem**: Docs hardcode micro-effect list (`timer/blob`) while enforcement uses `origin_scope` (spec/03-air.md §7; spec/04-reducers.md §“Anti-Patterns”; crates/aos-kernel/src/effects.rs:95-122).
- **Fix**: Update reducer/air text to define “micro-effects” = effects whose `origin_scope` allows reducers; keep list as informational example.

## 6) Align “pure modules” messaging with v1 scope
- ✅ **Problem**: Overview still stated pure modules ship in v1 while core spec says `module_kind` is `"reducer"` only (deferred).
- ✅ **Fix** (done): Updated `spec/01-overview.md` to say pure modules are deferred to v1.1+, keeping v1 `module_kind` = `"reducer"` only (spec/03-air.md already notes future `"pure"`).

## 7) Add schema for patches
- 🔴 **Problem**: Patch format is prose-only (spec/03-air.md §15); no JSON Schema alongside others.
- **Fix**: Add `spec/schemas/patch.schema.json` covering patch document + operations; link from §15 and wire into tooling validation if applicable.

## 8) Optional: make `required_caps` / `allowed_effects` derived-only
- 🟡 **Problem**: Fields persist in plans and are normalized (spec/03-air.md §12; spec/schemas/defplan.schema.json; crates/aos-air-types/src/validate.rs:99-144). This is redundant with `emit_effect` steps.
- **Fix (optional)**: Treat them as tooling-only projections (not stored/hased) or remove from schema and derive on load; update prose accordingly. If kept, document “redundant hint” status explicitly.

---

## Quick status table
- Require explicit `air_version`: ✅
- Remove built-in auto-inclusion: ✅
- Policy host/method removal: ✅
- Await-event correlation validation: ✅
- Micro-effect definition via `origin_scope`: 🟡 (code OK, docs lag)
- Pure modules messaging: ✅
- Patch schema: 🔴
- Derived caps/effects optionality: 🟡 (current behavior is “persist + validate”)
