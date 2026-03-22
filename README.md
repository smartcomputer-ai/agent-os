# AgentOS

**🌞 An agent harness for self-evolving agents.**

AgentOS is an agent harness designed for autonomous self-modification of both the agent and the harness around it. Agents can safely propose, simulate, and apply changes to their own code, policies, workflows, and runtime configuration under governance, with full audit trails. Every external action produces a signed receipt. Every state change is replayable from an event log.

## Why AgentOS

Agents today sit on stacks never designed for self-modification. State sprawls across systems, audits are partial, and governance is bolted on. 

AgentOS makes determinism and governed evolution first-class. Build portable, forkable worlds where agents own their runtime and every change is auditable.

## Architecture Overview

The current runtime is written in Rust and supports the following features:

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
3. **[spec/03-air.md](spec/03-air.md)** — Complete AIR v1 spec (schemas, modules, capabilities, policies, manifests)
4. **[spec/05-workflows.md](spec/05-workflows.md)** — Canonical workflow model and runtime semantics after plan removal
5. **[spec/06-cells.md](spec/06-cells.md)** — Keyed workflow instances, routing, scheduling, and storage
6. **[spec/07-gc.md](spec/07-gc.md)** — CAS reachability, snapshot roots, and garbage-collection semantics

For implementation guidance, project structure, and coding conventions, see **[AGENTS.md](AGENTS.md)**.

## Try AOS

AOS is not quite ready for daily use yet, but it is close. The main proof of concept today is the `Demiurge` agent. The repository also includes the `aos-smoke` crate, which exercises and demonstrates core AOS capabilities.

Before you get started, make sure you have the Rust toolchain installed.

### Try Demiurge Locally

`worlds/demiurge` is the task-driven local agent workflow in this repo. A simple happy path is:

If you want live LLM calls, set a provider API key first. You can either export it in your shell or
put it in `worlds/demiurge/.env`. Local Demiurge reads local secrets from env/`.env`; nothing is
stored in the world or local backend. For example:

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

3. In terminal 2, create and select the world and build from the local root:

```bash
target/debug/aos world create \
  --local-root worlds/demiurge \
  --force-build \
  --select \
  --verbose
```

4. Submit a task:

```bash
worlds/demiurge/scripts/demiurge_task.sh \
  --task "Summarize what this project is about, start with the README."
```

For more details, see [`worlds/demiurge/README.md`](worlds/demiurge/README.md).

### Try Demiurge With the Hosted Node

You can also run the Kafka broker-backed hosted runtime locally against the same world root. This keeps
the hosted node state under `worlds/demiurge/.aos-hosted` and points a saved hosted profile at the
local control API.

1. Build the hosted binary alongside the CLI and workflow artifacts:

```bash
rustup target add wasm32-unknown-unknown

cargo build -p aos-cli -p aos-node-hosted
cargo build -p aos-sys --target wasm32-unknown-unknown
cargo build -p aos-agent --bin session_workflow --target wasm32-unknown-unknown
```

2. Bring up the local hosted infra from `dev/`. This starts Redpanda (Kafka) on
   `localhost:19092`, MinIO, and creates the local topics:

```bash
dev/scripts/hosted-up.sh
```

For teardown, topic resets, and blobstore resets, see
[`dev/README-hosted.md`](dev/README-hosted.md).

3. Export the hosted runtime environment before starting `aos-node-hosted`:

```bash
export AOS_KAFKA_BOOTSTRAP_SERVERS=localhost:19092
export AOS_KAFKA_INGRESS_TOPIC=aos-ingress
export AOS_KAFKA_JOURNAL_TOPIC=aos-journal
export AOS_KAFKA_PROJECTION_TOPIC=aos-projection

export AOS_BLOBSTORE_BUCKET=aos-dev
export AOS_BLOBSTORE_ENDPOINT=http://localhost:19000
export AOS_BLOBSTORE_REGION=us-east-1
export AOS_BLOBSTORE_PREFIX=aos
export AOS_BLOBSTORE_FORCE_PATH_STYLE=true

export AOS_PARTITION_COUNT=1
export AWS_ACCESS_KEY_ID=minioadmin
export AWS_SECRET_ACCESS_KEY=minioadmin
```

Provider secrets such as `OPENAI_API_KEY` or `ANTHROPIC_API_KEY` should still live in
`worlds/demiurge/.env` (or whatever secret sources are configured by `aos.sync.json`). In hosted
mode those values are not read directly from the worker process environment at task runtime;
`--sync-secrets` uploads them into the hosted node secret store for the selected universe. If you
change `worlds/demiurge/.env` later, resync with:

```bash
target/debug/aos world patch \
  --local-root worlds/demiurge \
  --sync-secrets
```

4. In terminal 1, start the locally hosted node on the Demiurge world root:

```bash
target/debug/aos hosted up --root .aos-hosted --select
```

5. In terminal 2, create and select the hosted world and sync hosted secrets from
   `worlds/demiurge/.env`:

```bash
target/debug/aos world create \
  --local-root worlds/demiurge \
  --sync-secrets \
  --force-build \
  --select \
  --verbose
```

6. Submit a task:

```bash
worlds/demiurge/scripts/demiurge_task.sh \
  --task "Summarize what this project is about, start with the README."
```

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
