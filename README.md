# AgentOS

**🌞 An agent harness for self-evolving agents.**

AgentOS is an agent harness designed for autonomous self-modification of both the agent and the harness around it. Agents can safely propose, simulate, and apply changes to their own code, policies, workflows, and runtime configuration under governance, with full audit trails. Every external action produces a signed receipt. Every state change is replayable from an event log.

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
4. **[spec/04-workflows.md](spec/04-workflows.md)** — Workflow semantics, ABI, relationship to plans
5. **[spec/05-workflows.md](spec/05-workflows.md)** — Coordinating complex workflows (patterns, compensations, retries)

For implementation guidance, project structure, and coding conventions, see **[AGENTS.md](AGENTS.md)**.

## Try AOS

AOS is not quite ready for daily use yet, but it is close. The main proof of concept today is the `Demiurge` agent. The repository also includes the `aos-smoke` crate, which exercises and demonstrates core AOS capabilities.

Before you get started, make sure you have the Rust toolchain installed.

### Try Demiurge Locally

`worlds/demiurge` is the task-driven local agent workflow in this repo. A simple happy path is:

If you want live LLM calls, set a provider API key first. You can either export it in your shell or
put it in `worlds/demiurge/.env` so `--sync-secrets` can import it. For example:

```bash
export OPENAI_API_KEY=...
# or
export ANTHROPIC_API_KEY=...
```

1. Build the local debug binaries and workflow artifacts:

```bash
rustup target add wasm32-unknown-unknown

cargo build -p aos-cli -p aos-node-local
cargo build -p aos-sys --target wasm32-unknown-unknown
cargo build -p aos-agent --bin session_workflow --target wasm32-unknown-unknown
```

2. In terminal 1, start the local node on the Demiurge world root:

```bash
target/debug/aos local up --root worlds/demiurge --select
```

3. In terminal 2, create and select the world, build from the local root, and sync secrets from `worlds/demiurge/aos.sync.json`:

```bash
target/debug/aos world create \
  --local-root worlds/demiurge \
  --handle demiurge \
  --force-build \
  --select \
  --sync-secrets \
  --verbose
```

4. Submit a task:

```bash
worlds/demiurge/scripts/demiurge_task.sh \
  --task "Summarize what this project is about, start with the README."
```

For more details, see [`worlds/demiurge/README.md`](worlds/demiurge/README.md).

### Running the Examples

All ladder demos live under `crates/aos-smoke/fixtures/` and share the `aos-smoke` CLI.

- List demos: `cargo run -p aos-smoke --`
- Run a single demo (e.g., counter): `cargo run -p aos-smoke -- counter`
- Run them sequentially: `cargo run -p aos-smoke -- all`
- Force a rebuild of workflow WASM/artifacts: add `--force-build`, e.g. `cargo run -p aos-smoke -- --force-build counter`
- Increase logging by exporting `RUST_LOG=debug` before invoking the CLI if you need cache/build insight

## Current Status

AgentOS is in active development. We're building the architecture in the open and invite feedback and collaboration.

This version of AgentOS replaces our first attempt, which can be [found here](https://github.com/smartcomputer-ai/agent-os/tree/pre-next). That version was quite different in form, but similar in philosophy.

## Contributing

Feedback, questions, and contributions are welcome. Open an issue or start a discussion.

## License

AgentOS is open-source software licensed under the **Apache License 2.0**.
The runtime, kernel, adapters, and SDKs are available for free use and modification under that license, with an explicit grant of patent rights.

The AIR specification and schema documents are published under the **Creative Commons Attribution 4.0 International (CC BY 4.0)** license with a royalty-free patent non-assert, so anyone can build compatible implementations.

See [`LICENSE`](./LICENSE) and [`LICENSE-SPEC`](./LICENSE-SPEC) for full terms.
