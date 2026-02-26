# aos-agent-sdk AIR Assets

This directory holds canonical reusable AIR assets for the workflow-native Agent SDK.

- `schemas.air.json`: SDK contract schemas (`aos.agent/*` only)
- `module.air.json`: workflow/pure module definitions for SDK WASM binaries
- `manifest.air.json`: minimal manifest wiring for contract bootstrapping
- `capabilities.air.json`: placeholder for SDK capability templates
- `policies.air.json`: placeholder for SDK policy templates

Import policy:
- Consumers should import this AIR root directly (`air_dir: "air"`).
- Import loaders ignore manifest nodes in imported roots and only merge defs.
- No parallel `air/exports/*` mirrors are maintained; definitions live once here.
