# Example 06 — Safe Upgrade

This demo exercises the governance loop for upgrading a plan. It starts with
`demo/fetch_plan@1`, proposes a manifest that switches to `demo/fetch_plan@2`
(and adds a new HTTP capability + policy), runs a shadow prediction, and then
approves/applies the change before re-running the workflow.

* Reducer: `demo/SafeUpgrade@1` (WASM in `reducer/`)
* Plans: `demo/fetch_plan@1` (single HTTP) → `demo/fetch_plan@2` (adds follow-up HTTP)
* Capabilities: `demo/http_fetch_cap@1` plus new `demo/http_followup_cap@1`
* Policy upgrade: `demo/http-policy@1` → `demo/http-policy@2`

Layout:

- `air.v1/` — AIR v1 bundle (schemas, module, manifest, caps/policies, `fetch_plan@1`)
- `air.v2/` — upgraded AIR bundle (adds `fetch_plan@2`, new cap/policy) used for the proposal
- `reducer/` — Wasm reducer crate compiled via `aos-wasm-build`

Run it with:

```
cargo run -p aos-examples -- safe-upgrade
```
