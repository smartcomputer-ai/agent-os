# Example 03 — Fetch & Notify

Plan-driven demo that shows a reducer emitting a `FetchRequest` DomainIntent,
triggering a plan that performs an HTTP request, then raising a typed
`NotifyComplete` event back to the reducer.

Artifacts:

- `air/` — all AIR assets (schemas, reducer module, manifest, capabilities, policies, and plans)
- `reducer/` — Wasm reducer crate compiled via `aos-wasm-build`
- `defs/` — reserved for shared/builtin JSON definitions used by the example
