## 1. Patch schema feedback

### 1.1 Shape + concurrency model: 👍

* `base_manifest_hash` at the document level + `pre_hash` on `replace_def`/`remove_def` is exactly the right “optimistic concurrency” story.
* Keeping `node_json` structurally loose and delegating real validation to the loader is also correct; it matches the “schema-directed sugar vs canonical JSON” story in the AIR spec.

No changes needed here; just worth calling out that the invariants should be:

* When applying a patch doc, the kernel must verify `base_manifest_hash` still exists and is the manifest you’re patching.
* For each `replace_def`/`remove_def`, the referenced node’s hash must equal `pre_hash` at that manifest. If *any* pre‑hash check fails, the whole patch doc is rejected.

If these invariants are already enforced, you’re in good shape.

---

### 1.2 `kind` fields could be stricter

Right now every operation carries a `kind` which is just `type: "string"`:

* `add_def.add_def.kind`
* `replace_def.replace_def.kind`
* `remove_def.remove_def.kind`
* `set_manifest_refs.set_manifest_refs.add[].kind`
* `set_manifest_refs.set_manifest_refs.remove[].kind`

Given AIR treats “kind” as a closed set (`defschema`, `defmodule`, `defplan`, `defcap`, `defpolicy`, `defsecret`, `defeffect`, `manifest`), you could catch a lot of typos by tightening this to an enum or a shared `$defs/DefKind` in `common.schema.json`.

**Suggestion**

* Add `DefKind` to `common.schema.json` and reference it from patch.schema.
* If you want to keep room for `defmigration` later, you can add it to the enum when it lands.

Not critical, but it’s cheap correctness and better tooling autocomplete.

---

### 1.3 Manifest coverage: are routing/triggers/module_bindings intentionally out-of-scope?

Manifest shape includes:

* `schemas`, `modules`, `plans`, `effects`, `caps`, `policies`, `secrets`
* `defaults` (policy + cap_grants)
* `routing` (events/inboxes)
* `triggers`
* `module_bindings`

Patch ops currently give you:

* `add_def` / `replace_def` / `remove_def` – for all the def* nodes
* `set_manifest_refs` – which can cover refs for schemas/modules/plans/effects/caps/policies/secrets
* `set_defaults` – for `defaults.policy` and `defaults.cap_grants`

But there are **no** first‑class ops for:

* `routing.events`
* `routing.inboxes`
* `triggers`
* `module_bindings`

So today, governance patches can’t change routes/triggers/bindings except by some out‑of‑band mechanism. That’s slightly at odds with the “all control‑plane changes expressed as AIR patches” story in the architecture/spec.

This might be intentional (v1 patches only for refs+defaults, more ops later), but if you *do* want patch docs to be the one true path for routing changes, I’d consider:

* Adding explicit ops like:

  * `set_routing_events`
  * `set_routing_inboxes`
  * `set_triggers`
  * `set_module_bindings`
* Or a more generic `replace_manifest_block` with a pre‑hash on the manifest and a limited subset of fields allowed.

At minimum, I’d call this out explicitly in the spec:

> v1 patches can change defs, manifest refs, and defaults; routing/triggers/module_bindings changes are deferred to v1.1 and may use a separate governance surface.

Otherwise people will assume they can govern everything through patches and hit a wall when they need to add a trigger.

---

### 1.4 `set_manifest_refs` small ergonomics nit

`set_manifest_refs` requires `add` but makes `remove` optional:

```json
"properties": {
  "set_manifest_refs": {
    "properties": {
      "add": { ... },
      "remove": { ... }
    },
    "required": ["add"]
  }
}
```

So a remove‑only patch has to be:

```json
{"set_manifest_refs": { "add": [], "remove": [ ... ] }}
```

Totally workable, but a bit surprising for hand‑authored docs.

**Suggestion**

* Either:

  * Make both `add` and `remove` optional in the schema and define the “at least one non‑empty” invariant in prose / runtime validation; or
  * Keep the schema as‑is and explicitly document the “use `add: []` for remove‑only” sugar in the patch section of the spec.

Not a correctness issue, just ergonomics.

---

### 1.5 `set_defaults` semantics: document the tri‑state clearly

The shape:

```json
"set_defaults": {
  "properties": {
    "policy": { "oneOf": [ Name, null ] },
    "cap_grants": [ CapGrant... ]
  }
}
```

 

Implies the following semantics:

* `policy` omitted → leave as‑is
* `policy` = Name → set default policy to that defpolicy
* `policy` = null → clear default policy

I think that’s exactly what you want, but it would be good to codify that explicitly in the spec text and in the P5 doc, so people don’t treat `null` as “no change”.

Same for `cap_grants`:

* Omitted → no change
* Present but `[]` → clear defaults
* Present and non‑empty → replace with this new list

That gives you a nice “PATCH‑like” semantics; you already have the schema to support it.

---

### 1.6 `node_json` description vs manifest

`node_json` is described as:

> Authoring form of any AIR node (defschema, defmodule, defplan, defeffect, defcap, defpolicy, defsecret).

So the manifest is explicitly *not* in scope for `add_def` / `replace_def`. That’s fine, but then the **only** way to mutate the manifest is the dedicated ops (`set_manifest_refs` / `set_defaults`). That makes the omission of routing/triggers/bindings even more significant (see 1.3).

If that’s intentional for v1, I’d:

* Make that boundary explicit in spec/03‑air.md §15 (patches only touch those fields of the manifest for now).
* Add a small “future work” bullet listing `set_routing` / `set_triggers` / `set_module_bindings` as planned patch ops.

---

### 1.7 Patch versioning & forward‑compat

Since the patch schema is hosted at `air/v1/patch.schema.json` and `patches[].items` is a `oneOf` over the five current ops, older kernels won’t accept newer ops once you add them.

That’s probably fine (governance changes are usually upgrade‑gated anyway), but you may want:

* A `version` or `air_version` field on the patch document itself, mirroring the manifest’s `air_version: "1"` field.
* A clear rule: “v1 kernels reject patch docs whose `version` they don’t understand”, so you can introduce `v2` later if you need more invasive changes.

Not urgent, but cheap to add now.

---

## 2. Governance / control-surface feedback

The P5 doc overall feels very coherent with the rest of the system: proposals as patch docs, explicit governance verbs, and typed effect schemas for the self‑upgrade path.

A few specific points:

### 2.1 Gov effect schemas look good; I’d add one field

Reserved params/receipts:

* `sys/GovProposeParams@1` / `Receipt@1`
* `sys/GovShadowParams@1` / `Receipt@1`
* `sys/GovApproveParams@1` / `Receipt@1`
* `sys/GovApplyParams@1` / `Receipt@1`

This lines up nicely with the constitutional loop already described in the architecture/spec.

One small tweak I’d consider:

* `GovApproveParams@1` and/or `GovApproveReceipt@1` including an optional `reason?:text` (or `rationale_ref?:hash`) in addition to `approver:text`.

Right now you only capture `decision` and `approver`. Having a place for “why did we approve/reject this?” will be extremely useful for audits later, and it’s much nicer if that’s a first‑class field rather than encoded in some off‑to‑the‑side log.

### 2.2 Governance cap type schema is nice

The proposed `sys/governance@1`:

```json
{
  "$kind":"defcap",
  "name":"sys/governance@1",
  "cap_type":"governance",
  "schema":{
    "record":{
      "modes":{ "set":{ "text":{} } },
      "namespaces":{ "set":{ "text":{} } },
      "max_patches":{ "nat":{} }
    }
  }
}
```



This is a good start. A couple of clarifications I’d bake into docs/spec:

* Define `modes` as an enum-like set: e.g. `"propose" | "shadow" | "approve" | "apply"` strings (you’re already hinting at this, just make it explicit).
* Define `namespaces` precisely as “the `namespace/` prefix portion of a Name”, so people know how to scope e.g. `com.acme/*`.
* Clarify how `max_patches` interacts with proposals that contain multiple ops: is it “number of patch docs this principal can propose overall” or “maximum number of *ops* per patch doc”? The doc currently says “optional ceiling for proposals” which sounds like the former, but the field name is per‑cap, so I’d clarify.

You might eventually want to add another dimension like:

* `kinds?: set<text>` → allowed defkinds (`defmodule`, `defplan`, …) that this governance capability can touch (e.g. “let this actor only touch policies and caps, but not modules”).

But what you have is already useful.

### 2.3 Patch base vs governance params: avoid duplicate sources of truth

Patch docs carry `base_manifest_hash`.

Your proposed `GovProposeParams@1` also carries `manifest_base?:hash`.

That gives you two fields that can describe the same thing. I’d define a clear rule:

* `GovProposeParams.manifest_base` **must** equal `patch.base_manifest_hash` when present; otherwise the proposal is invalid.
* If `manifest_base` is omitted, the runtime infers it from the patch doc (and/or fills it into the params in receipts).

That keeps the “what manifest was this patch authored against?” answer unambiguous across patch docs and governance entries.

### 2.4 Journaling invariants: agree with your “must mirror” note

The P5 doc says:

> Receipts emitted by governance effects must mirror the canonical governance journal entries (Proposed/ShadowReport/Approved/Applied) so replay remains deterministic; journal stays the source of truth.

Strong +1 to this. In particular:

* `GovShadowReceipt@1` mirroring whatever you store as the internal `ShadowReport` journal entry.
* `GovApplyReceipt@1` carrying `manifest_hash_new` which must match the journal’s view.

I’d make this norm explicit in the AIR spec too: “Governance receipts are a *view* over the canonical governance journal entries; discrepancies are a bug.”

---

## 3. CLI / UX & ergonomics

Most of this is already in your “Proposed work” + TODOs, but a few concrete suggestions:

### 3.1 `--patch-dir` + hashless authoring: nail down the rules

The doc mentions:

* Accepting “hashless” assets with ZERO_HASH wasm placeholders and missing manifest ref hashes.
* CLI path that loads nodes, stores them, fills hashes, patches manifest refs, then canonicalizes & hashes the patch doc before submission.

I’d make the CLI behavior very explicit:

1. `aos world gov propose --patch-dir <dir> --base <hash?>`:

   * Load AIR bundle from `<dir>`.
   * Canonicalize and store all defs; compute their hashes.
   * Compute a patch doc against `--base` (or world head) that:

     * Uses `add_def`/`replace_def`/`remove_def` for defs.
     * Uses `set_manifest_refs` for manifest lists.
     * Uses `set_defaults` if needed for policy/cap_grants.
   * Validate patch doc against `patch.schema.json`.
   * Show a human‑readable summary/diff (like “add 2 defmodule, replace 1 defplan, set_defaults.policy→X, add manifest.refs: com.acme/foo@1”).
   * On confirmation, submit patch doc.

2. When `--base` is omitted:

   * Fill `base_manifest_hash` from current world head.
   * Still bake that into the patch doc so it’s replayable later.

3. `--require-hashes`:

   * Forbid ZERO_HASH placeholders and missing manifest entry hashes in inputs; fail fast.

All of this is implied in P5, but putting it in the CLI docs/spec would reduce surprises.

### 3.2 `--dry-run` output: include resolved hashes

For `--dry-run` on `--patch-dir`, it’s really helpful to:

* Print the patch doc *with* all hashes filled in (what will actually be submitted).
* Optionally print a tiny “manifest head → manifest_new” summary: e.g., `sha256:abc → sha256:def` plus counts of def kinds updated.

This makes it much easier to debug “why did my ZERO_HASH placeholder turn into *this* hash?”

### 3.3 Error reporting: use optional fields on the patch doc sparingly

Your design note says:

> If richer error info is needed, extend the schema with optional fields rather than inventing alternate payload shapes.

I think that’s the right instinct. If you later decide you want to carry, say, an `origin` or `span` for each patch op (e.g., “came from file X:line Y”), I’d recommend:

* Add a generic optional `meta?: { origin?: text, note?: text }` on each op type rather than sprinkling different ad‑hoc fields.

But I wouldn’t add that *now* unless you already have a concrete use for it; it’s easy to extend later without breaking anything.

---

## 4. “Did we forget anything?” – short list

Boiling it down to the stuff I’d most seriously consider changing or at least documenting:

1. **Routing/triggers/module_bindings patching**

   * Either:

     * Add explicit patch ops for these manifest fields, or
     * Clearly document that v1 governance patches can’t touch them and that they’ll be handled by a future patch‑schema extension.

2. **Tighten `kind`**

   * Add a shared `DefKind` enum and reference it in patch.schema so typos are caught early.

3. **Clarify `set_defaults` & `set_manifest_refs` semantics**

   * Document the tri‑state (`omit` vs `null` vs `[]`) for defaults.
   * Decide whether you want to allow remove‑only `set_manifest_refs` without the `add: []` hack, or at least document the hack.

4. **Make patch base vs governance base consistent**

   * Define a single source of truth for base manifest in proposals: `GovProposeParams.manifest_base` must match `patch.base_manifest_hash`, or it’s invalid.

5. **Optional: add `reason` to approvals**

   * Add an optional `reason`/`rationale` field to `GovApproveParams` / `GovApproveReceipt` so you don’t lose human explanations for decisions.

If you do just those, I think the patch + governance story will feel very “finished” and line up tightly with the rest of AIR/AgentOS.

If you want, I can also mock up candidate JSON snippets for `set_routing_*` / `set_triggers` / `set_module_bindings` ops that match the style of the existing patch schema.
