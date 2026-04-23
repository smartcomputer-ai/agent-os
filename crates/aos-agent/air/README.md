# aos-agent AIR Assets

This directory holds reusable AIR assets for the workflow-native agent package.

- `schemas.air.json`: contract schemas (`aos.agent/*` only)
- `module.air.json`: workflow/pure module definitions for agent WASM binaries
- `manifest.air.json`: minimal manifest wiring for contract bootstrapping
- `generated/`: Rust-authored AIR emitted by `cargo run -p aos-agent --bin aos-air-export`

The Rust contract and workflow definitions are the authoring source for generated AIR. During the
transition, the hand-authored files remain checked in beside `generated/`; duplicate defs are
expected to hash-identically.

Import policy:
- Consumers should import this AIR root directly (`air_dir: "air"`).
- Import loaders ignore manifest nodes in imported roots and only merge defs.
- Generated and hand-authored defs must stay equivalent while both are present.
