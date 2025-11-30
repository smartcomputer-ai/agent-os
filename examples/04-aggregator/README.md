# Example 04 — Aggregator (fan-out + join)

This rung fans out three HTTP requests through a plan, waits for their
receipts out of order, and then raises an `AggregateComplete` event back
to the reducer. The reducer tracks pending fan-out work keyed by
`request_id`, emits the `AggregateRequested@1` intent (including the
per-target method/URL/name supplied in the `Start` event), and stores the
response summaries for every target once the plan rejoins.

Artifacts:

- `air/` — canonical JSON AIR assets (schemas, reducer module, manifest,
  capabilities, policies, and plans)
- `reducer/` — Wasm reducer crate compiled via `aos-wasm-build`
- `defs/` — reserved for shared/builtin JSON definitions
