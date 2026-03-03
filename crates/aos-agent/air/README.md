# aos-agent AIR Assets

This directory holds canonical reusable AIR assets for the workflow-native agent package.

- `schemas.air.json`: contract schemas (`aos.agent/*` only)
- `module.air.json`: workflow/pure module definitions for agent WASM binaries
- `manifest.air.json`: minimal manifest wiring for contract bootstrapping
- `capabilities.air.json`: placeholder for capability templates
- `policies.air.json`: placeholder for policy templates

Import policy:
- Consumers should import this AIR root directly (`air_dir: "air"`).
- Import loaders ignore manifest nodes in imported roots and only merge defs.
- No parallel `air/exports/*` mirrors are maintained; definitions live once here.
