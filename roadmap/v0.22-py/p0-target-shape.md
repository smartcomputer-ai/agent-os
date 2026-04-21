# P0: Target Shape

Status: planned.

## Goal

Lock the defop refactor target before touching code. The rest of the work should move directly toward this shape instead of preserving legacy AIR compatibility.

The target public surface for v0.22 is:

```text
defschema   = data shapes
defmodule   = executable bundle / runtime artifact
defop       = typed executable entrypoint
defsecret   = secret declaration
manifest    = active schemas + modules + ops + secrets + routing
```

No public caps.
No public policies.
No `defeffect` as a canonical root form.
No `manifest.effect_bindings`.
No `manifest.module_bindings`.

## Decisions

1. `defmodule` is only runtime/artifact metadata.

   It should not carry workflow/pure ABI fields. WASM, Python, JS, builtin, and later native/remote packages all fit under one runtime object.

2. `defop` is the only callable operation definition.

   Workflow reducers, pure functions, effect handlers, builtin effects, future Python effects, and future authority helpers are all ops with different `op_kind` values.

3. Workflow routing targets ops.

   `routing.subscriptions[].op` replaces `routing.subscriptions[].module`.

4. Effect emission targets effect ops.

   `workflow.effects_emitted[]` lists effect op names, not semantic effect kind strings.

5. Effect semantic `kind` remains metadata.

   Keep `effect.kind = "http.request"` for adapter matching, observability, and future policy matching, but do not use it as canonical typed operation identity.

6. Origin scope is explicit.

   Use an array such as `["workflow", "system", "governance"]`. Do not keep `plan` or `both` in the public AIR shape.

7. Python support depends on op identity.

   Python effects should not be built on top of the old `EffectKind` plus `adapter_id` routing path.

## Open Details To Settle In This Phase

- Exact `defmodule.runtime` shape for WASM, Python, and builtin modules.
- Exact `defop.impl` shape.
- Exact `defop.workflow.determinism` values for v0.22.
- Whether `manifest.op_bindings` exists at all in v0.22. Recommendation: omit it unless it carries non-authority runtime config needed immediately.
- Whether the AIR version becomes `"2"` now. Recommendation: yes, because there is no compatibility promise on this branch.

## Done When

- `defop-idea.md` and `py-idea.md` agree with the simplified no-cap/no-policy v0.22 surface.
- The planned schema shape for `defmodule`, `defop`, and `manifest` is written down before implementation.
- The team accepts that old manifests and `defeffect` files can break.

