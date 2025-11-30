# Polish 2: Close the Remaining Spec/Runtime Gaps

Status legend: ✅ already aligned | 🟡 partially | 🔴 not yet

---

## 1) Require explicit `air_version` (stop “assume latest”)
- 🔴 **Problem**: `air_version` is optional and defaults to current major (spec/03-air.md lines ~110–129; spec/schemas/manifest.schema.json; crates/aos-store/src/manifest.rs:103-114). Future majors would silently upgrade old manifests.
- **Fix**:
  - Make `air_version` required and `enum: ["1"]` in `spec/schemas/manifest.schema.json` (add to `required`).
  - Update prose in `spec/03-air.md` §4 to remove “assume latest” sentence.
  - Keep loader behavior consistent: error on missing/unknown version (adjust `ensure_air_version` in `crates/aos-store/src/manifest.rs`).

## 2) Remove built-in auto-inclusion magic
- 🔴 **Problem**: Runtime/tooling auto-attaches built-in schemas/effects (spec/03-air.md lines ~129, 388; crates/aos-store/src/manifest.rs:260-289; crates/aos-kernel/src/manifest.rs:64-109), so manifest isn’t the single source of truth.
- **Fix**:
  - Drop auto-attach in loaders/kernels; require built-ins to be explicitly listed in `manifest.effects` and `manifest.schemas` (hashes already in `spec/defs/builtin-*.air.json`).
  - Update docs in `spec/03-air.md` §§4,7 accordingly.
  - Add authoring/tooling step (e.g., `aos world init`) that inserts built-ins once so files stay explicit.

## 3) Simplify policy match surface (drop host/method)
- 🔴 **Problem**: `defpolicy.Match` still contains HTTP-specific `host` and `method` (spec/03-air.md §11; spec/schemas/defpolicy.schema.json lines 40-59), overlapping with CapGrant constraints.
- **Fix**:
  - Remove `host`/`method` fields from schema and prose; leave `effect_kind`, `cap_name`, `origin_kind`, `origin_name` as the v1 surface.
  - Update any examples/tests that reference `host`/`method`.

## 4) Validate `await_event` correlation at authoring time
- 🟡 **Problem**: Runtime rejects missing `where` when `correlate_by` is set (crates/aos-kernel/src/plan.rs:327-333), but validator does not enforce presence/reference (crates/aos-air-types/src/validate.rs:167-212).
- **Fix**:
  - In validator: if manifest has any trigger with `correlate_by`, require every `await_event` in that plan to include `where`; optionally ensure the predicate references the correlation key.
  - Mirror this rule in `spec/03-air.md` §12 semantics.

## 5) Make micro-effect rule point to `origin_scope`
- 🟡 **Problem**: Docs hardcode micro-effect list (`timer/blob`) while enforcement uses `origin_scope` (spec/03-air.md §7; spec/04-reducers.md §“Anti-Patterns”; crates/aos-kernel/src/effects.rs:95-122).
- **Fix**: Update reducer/air text to define “micro-effects” = effects whose `origin_scope` allows reducers; keep list as informational example.

## 6) Align “pure modules” messaging with v1 scope
- 🔴 **Problem**: Overview still states pure modules ship in v1 (spec/01-overview.md lines ~78-88) while core spec says `module_kind` is `"reducer"` only (spec/03-air.md §6 notes deferred).
- **Fix**: Edit overview to say pure modules are deferred to v1.1+; keep enum extensibility note in §6.

## 7) Add schema for patches
- 🔴 **Problem**: Patch format is prose-only (spec/03-air.md §15); no JSON Schema alongside others.
- **Fix**: Add `spec/schemas/patch.schema.json` covering patch document + operations; link from §15 and wire into tooling validation if applicable.

## 8) Optional: make `required_caps` / `allowed_effects` derived-only
- 🟡 **Problem**: Fields persist in plans and are normalized (spec/03-air.md §12; spec/schemas/defplan.schema.json; crates/aos-air-types/src/validate.rs:99-144). This is redundant with `emit_effect` steps.
- **Fix (optional)**: Treat them as tooling-only projections (not stored/hased) or remove from schema and derive on load; update prose accordingly. If kept, document “redundant hint” status explicitly.

---

## Quick status table
- Require explicit `air_version`: 🔴
- Remove built-in auto-inclusion: 🔴
- Policy host/method removal: 🔴
- Await-event correlation validation: 🟡 (runtime only)
- Micro-effect definition via `origin_scope`: 🟡 (code OK, docs lag)
- Pure modules messaging: 🔴
- Patch schema: 🔴
- Derived caps/effects optionality: 🟡 (current behavior is “persist + validate”)

