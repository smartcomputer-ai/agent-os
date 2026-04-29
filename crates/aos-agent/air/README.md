# aos-agent AIR Assets

This directory holds reusable AIR assets for the workflow-native agent package.

- `generated/`: Rust-authored AIR emitted by `cargo run -p aos-agent --bin aos-air-export`

## Runtime Shape

`aos-agent` is a core session/run SDK, not a preconfigured software-factory agent. The default
session state has no tools, no tool profiles, and no host auto-open target. Embedding worlds choose
the tool surface explicitly by installing a registry/profile through Rust helpers or
`aos.agent/SessionInput@1`.

Built-in SDK bundles are optional:

1. inspect tools,
2. local host tools,
3. sandbox/Fabric-ready host tools,
4. workspace tools.

The generated `aos.agent/SessionWorkflow@1` AIR is the broad reusable adapter surface. It declares
the LLM, blob, host, introspect, and workspace effects needed by all built-in bundles, but those
effects are not exposed to the model unless the embedding world installs matching tools. Importing
this AIR root therefore makes the workflow available; it does not imply host or workspace access by
itself.

Host session auto-open is also explicit. Local and sandbox targets are represented by
`aos.agent/HostSessionOpenConfig@1`; if no session/run config provides one, the workflow can still
run as chat-only or workspace-only and will not emit `sys/host.session.open@1` automatically.

Common assembly shapes:

1. chat-only: import the workflow and leave the tool registry empty,
2. inspect-only: install `tool_bundle_inspect()`,
3. local coding: install the explicit `local_coding_agent_*` compatibility helpers and a local host config,
4. sandbox host: install host bundles and provide a sandbox `HostSessionOpenConfig`,
5. workspace-only: install `tool_bundle_workspace()` with no host config.

The Rust contract and workflow definitions are the authoring source. Keep generated AIR current with:

```sh
aos air generate --world-root crates/aos-agent --manifest-path crates/aos-agent/Cargo.toml --bin aos-air-export
aos air check --world-root crates/aos-agent --manifest-path crates/aos-agent/Cargo.toml --bin aos-air-export
```

Import policy:
- Consumers should import this AIR root directly (`air_dir: "air"`); the loader scans `generated/`.
- Import loaders ignore manifest nodes in imported roots and only merge defs.
