## 1. Patch schema feedback
**Status: implemented (P5).** Schema, compiler, CLI, and docs now reflect the changes noted below. Remaining work is deferred to p1 self-upgrade (governance effect adapter + minor CLI/docs polish).

### 1.1 Shape + concurrency model: 👍

* `base_manifest_hash` at the document level + `pre_hash` on `replace_def`/`remove_def` is exactly the right “optimistic concurrency” story.
* Keeping `node_json` structurally loose and delegating real validation to the loader is also correct; it matches the “schema-directed sugar vs canonical JSON” story in the AIR spec.

No changes needed here; just worth calling out that the invariants should be:

* When applying a patch doc, the kernel must verify `base_manifest_hash` still exists and is the manifest you’re patching.
* For each `replace_def`/`remove_def`, the referenced node’s hash must equal `pre_hash` at that manifest. If *any* pre‑hash check fails, the whole patch doc is rejected.

If these invariants are already enforced, you’re in good shape.

---

### 1.2 `kind` fields could be stricter

Status: **fixed**. Patch schema now uses a shared `DefKind` enum in `common.schema.json`; all patch `kind` fields reference it.

Rationale: catches typos and keeps tooling aligned with the closed def-kind set (`defschema`, `defmodule`, `defplan`, `defcap`, `defpolicy`, `defsecret`, `defeffect`, `manifest`).

---

### 1.3 Manifest coverage: are routing/triggers/module_bindings intentionally out-of-scope?

Status: **implemented**. Patch schema now includes block-level ops:
`set_routing_events`, `set_routing_inboxes`, `set_triggers`, `set_module_bindings`, and `set_secrets` (full replace with `pre_hash`; empty clears). Compiler applies them with optimistic concurrency.

---

### 1.4 `set_manifest_refs` small ergonomics nit

Status: **fixed**. Schema now allows either `add` or `remove` (or both); remove‑only patches no longer need `add: []`. Runtime still expects at least one entry.

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

Status: **fixed** in compiler + tests. Semantics are now:

* `policy` omitted → no change
* `policy` = Name → set
* `policy` = null → clear
* `cap_grants` omitted → no change
* `cap_grants` = [] → clear
* `cap_grants` = [grants…] → replace

---

### 1.6 `node_json` description vs manifest

`node_json` is described as:

> Authoring form of any AIR node (defschema, defmodule, defplan, defeffect, defcap, defpolicy, defsecret).

So the manifest is explicitly *not* in scope for `add_def` / `replace_def`. That’s fine, but then the **only** way to mutate the manifest is the dedicated ops (`set_manifest_refs` / `set_defaults`). That makes the omission of routing/triggers/bindings even more significant (see 1.3).

Update: manifest blocks are now patchable via dedicated ops (routing/triggers/module_bindings/secrets).

---

### 1.7 Patch versioning & forward‑compat

Status: **fixed**. PatchDocument now has `version: "1"` (defaulted); kernels reject unsupported versions. CLI emits it.

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

Status: **fixed**. `GovApproveParams@1` and `GovApproveReceipt@1` now include optional `reason:text`.

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

Rule **adopted**: `GovProposeParams.manifest_base`, when supplied, must equal `patch.base_manifest_hash`; proposals should be rejected on mismatch. Documented in spec; enforcement will live in the governance effect adapter/control path.

That keeps the “what manifest was this patch authored against?” answer unambiguous across patch docs and governance entries.

### 2.4 Journaling invariants: agree with your “must mirror” note

The P5 doc says:

> Receipts emitted by governance effects must mirror the canonical governance journal entries (Proposed/ShadowReport/Approved/Applied) so replay remains deterministic; journal stays the source of truth.

Strong +1 to this. In particular:

* `GovShadowReceipt@1` mirroring whatever you store as the internal `ShadowReport` journal entry.
* `GovApplyReceipt@1` carrying `manifest_hash_new` which must match the journal’s view.

I’d make this norm explicit in the AIR spec too: “Governance receipts are a *view* over the canonical governance journal entries; discrepancies are a bug.”



---

## 3. “Did we forget anything?” – short list

Boiling it down to the stuff I’d most seriously consider changing or at least documenting:

1. **Routing/triggers/module_bindings patching**

   * Implemented via dedicated set_* block ops with pre_hash guard.

2. **Tighten `kind`**

   * Done via `DefKind` enum in `common.schema.json`; patch.schema now references it.

3. **Clarify `set_defaults` & `set_manifest_refs` semantics**

   * Tri‑state implemented; remove-only allowed; semantics documented here and in spec.

4. **Make patch base vs governance base consistent**

   * Rule adopted; needs enforcement in governance adapter/control path.

5. **Optional: add `reason` to approvals**

   * Implemented (`reason:text` optional).

If you do just those, I think the patch + governance story will feel very “finished” and line up tightly with the rest of AIR/AgentOS.

If you want, I can also mock up candidate JSON snippets for `set_routing_*` / `set_triggers` / `set_module_bindings` ops that match the style of the existing patch schema.

## Action plan (draft)
**Done in P5**: DefKind enum; patch version field + rejection; tri-state `set_defaults`; remove-only `set_manifest_refs`; approval rationale; base-manifest rule documented; block ops for routing events/inboxes, triggers, module_bindings, secrets; defsecret support; CLI emits version and includes new blocks.

**Remaining / deferred (p1 self-upgrade)**  
- Governance effect adapter: handle `governance.*` intents in-kernel, enforce `manifest_base == base_manifest_hash`, mirror journal receipts so plans/reducers can drive upgrades.  
- CLI/docs polish: mention patch `version` and approval `reason`; add remove-only `set_manifest_refs` and block-op coverage tests.  
- Evaluate if we need per-entry pre_hash granularity or partial-merge ops; current ops are full replace with block pre_hash.
