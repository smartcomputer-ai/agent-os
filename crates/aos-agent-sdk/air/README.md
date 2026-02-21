# aos-agent-sdk AIR Assets

This directory holds canonical reusable AIR assets for the Agent SDK.

- `schemas.air.json`: P2.1/P2.2 contract schemas (`aos.agent/*` only)
- `module.air.json`: reducer/pure module definitions for SDK WASM binaries
- `manifest.air.json`: minimal manifest wiring for contract bootstrapping
- `capabilities.air.json`: placeholder for SDK capability templates
- `policies.air.json`: placeholder for SDK policy templates
- `plans/`: reusable plan templates (for example workspace sync)
  including sync-time workspace JSON validation inputs.
- `exports/session-contracts/defs.air.json`: defs-only export for app/world imports
