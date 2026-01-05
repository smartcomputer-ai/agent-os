# AgentOS

**🌞 A runtime for self-evolving agents.**

AgentOS is a runtime where agents can safely propose, simulate, and apply changes to their own code, policies, and workflows, all under governance, with full audit trails. Every external action produces a signed receipt. Every state change is replayable from an event log.

## Why AgentOS

Agents today sit on stacks never designed for self-modification. State sprawls across systems, audits are partial, and governance is bolted on. 

AgentOS makes determinism and governed evolution first-class. Build portable, forkable worlds where agents own their runtime and every change is auditable.

## Our Architecture in Short

The current experimental runtime is written in Rust and supports the following features:

- **Deterministic kernel**: Single-threaded worlds with replay-identical state
- **AIR (Agent Intermediate Representation)**: Typed control plane for modules, plans, schemas, policies, and capabilities (homoiconic in spirit, where agents can read and edit their own runtime)
- **Capability security**: No ambient authority. All effects are scoped, budgeted, and gated by policy
- **Full auditability**: Signed receipts for every external action enable complete forensic replay
- **Safe self-modification**: Governed evolution through a constitutional loop that works as follows:
  1. **propose**: Draft changes to code, policies, or workflows
  2. **shadow**: Simulate changes in isolated environment
  3. **approve**: Policy-gated authorization
  4. **apply**: Atomically update the world state
  5. **execute**: Run effects with capability constraints
  6. **receipt**: Capture signed outcomes
  7. **audit**: Full provenance from intent to effect

## Documentation

Start here:

1. **[spec/01-overview.md](spec/01-overview.md)** — Core concepts, mental model, why this exists
2. **[spec/02-architecture.md](spec/02-architecture.md)** — Runtime components, event flow, storage layout
3. **[spec/03-air.md](spec/03-air.md)** — Complete AIR v1 spec (schemas, modules, plans, capabilities, policies)
4. **[spec/04-reducers.md](spec/04-reducers.md)** — Reducer semantics, ABI, relationship to plans
5. **[spec/05-workflows.md](spec/05-workflows.md)** — Coordinating complex workflows (patterns, compensations, retries)

For implementation guidance, project structure, and coding conventions, see **[AGENTS.md](AGENTS.md)**.

## Running the Examples

All ladder demos live under `examples/` and share the `aos-examples` CLI.

- List demos: `cargo run -p aos-examples --`
- Run a single demo (e.g., counter): `cargo run -p aos-examples -- counter`
- Run them sequentially: `cargo run -p aos-examples -- all`
- Force a rebuild of reducer WASM/artifacts: add `--force-build`, e.g. `cargo run -p aos-examples -- --force-build counter`
- Increase logging by exporting `RUST_LOG=debug` before invoking the CLI if you need cache/build insight

## Workspaces and Sync

AgentOS stores code and artifacts in **workspaces**: versioned trees managed by the built-in `sys/Workspace@1` reducer. Use `aos ws` for inspection/editing and `aos push`/`aos pull` to sync local directories via `aos.sync.json`.

Examples:
- `aos ws ls`
- `aos ws cat alpha/README.txt`
- `aos push`
- `aos pull`

## Current Status

AgentOS is in active development. We're building the architecture in the open and invite feedback and collaboration.

This version of AgentOS replaces our first attempt, which can be [found here](https://github.com/smartcomputer-ai/agent-os/tree/pre-next), and which was quite different in nature but same in philosphy.

## Contributing

Feedback, questions, and contributions are welcome. Open an issue or start a discussion.

## License

AgentOS is open-source software licensed under the **Apache License 2.0**.
The runtime, kernel, adapters, and SDKs are available for free use and modification under that license, with an explicit grant of patent rights.

The AIR specification and schema documents are published under the **Creative Commons Attribution 4.0 International (CC BY 4.0)** license with a royalty-free patent non-assert, so anyone can build compatible implementations.

See [`LICENSE`](./LICENSE) and [`LICENSE-SPEC`](./LICENSE-SPEC) for full terms.
