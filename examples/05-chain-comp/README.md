# Example 05 — Chain + Compensation (M4 multi-plan choreography)

This rung demonstrates reducer-driven sagas that stitch multiple plans
into a deterministic choreography:

1. `charge_plan` handles payment authorization
2. `reserve_plan` reserves inventory
3. `notify_plan` emits a downstream notification when everything succeeds
4. `refund_plan` compensates the charge when the reservation fails

The reducer keeps track of a `request_id` correlation key, emits
`*_Requested@1` intents with matching keys, and triggers the refund plan
if a `ReserveFailed` event arrives. Plans reuse the shared HTTP harness so
we can respond with synthetic receipts and exercise the failure path.

Artifacts:

- `air/` — canonical JSON AIR assets (schemas, reducer module, manifest,
  capabilities, policies, and plans: `charge`, `reserve`, `notify`,
  `refund`)
- `reducer/` — Wasm reducer crate compiled via `aos-wasm-build`
