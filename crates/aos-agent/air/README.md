# aos-agent AIR Assets

This directory holds reusable AIR assets for the workflow-native agent package.

- `generated/`: Rust-authored AIR emitted by `cargo run -p aos-agent --bin aos-air-export`

The Rust contract and workflow definitions are the authoring source. Keep generated AIR current with:

```sh
aos air generate --world-root crates/aos-agent --manifest-path crates/aos-agent/Cargo.toml --bin aos-air-export
aos air check --world-root crates/aos-agent --manifest-path crates/aos-agent/Cargo.toml --bin aos-air-export
```

Import policy:
- Consumers should import this AIR root directly (`air_dir: "air"`); the loader scans `generated/`.
- Import loaders ignore manifest nodes in imported roots and only merge defs.
