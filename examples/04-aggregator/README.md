# Example 04 — Aggregator (fan-out + join)

This rung fans out three HTTP requests through a plan, waits for their
receipts out of order, and then raises an `AggregateComplete` event back
to the reducer. The reducer tracks pending fan-out work keyed by
`request_id`, emits the `AggregateRequested@1` intent, and stores the
status/body previews for every response once the plan rejoins.

Artifacts:

- `air/` — canonical JSON AIR assets (schemas, reducer module, manifest,
  capabilities, and policies)
- `plans/` — AIR `defplan` definitions used by this demo
- `reducer/` — Wasm reducer crate compiled via `aos-wasm-build`
- `defs/` — reserved for shared/builtin JSON definitions
