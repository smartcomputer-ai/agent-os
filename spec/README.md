# AgentOS Specs

This directory is the source-of-truth shelf for active AgentOS protocol and runtime specs.

## Active Specs

1. **[01-overview.md](01-overview.md)** — Core concepts and mental model.
2. **[02-architecture.md](02-architecture.md)** — Runtime components, storage layout, execution, and governance.
3. **[03-air.md](03-air.md)** — AIR v1 control-plane IR, schemas, manifests, effects, capabilities, policies, and patch format.
4. **[04-workflows.md](04-workflows.md)** — Workflow module runtime contract and orchestration patterns.
5. **[05-effects.md](05-effects.md)** — Effects, async execution, durable open work, and continuation admission.

## Future Protocol Notes

- **[20-gc.md](20-gc.md)** — Draft reachability and future GC contract. Mark/sweep deletion is not implemented yet.

## Reference Shelves

- **[schemas/](schemas/)** — JSON Schemas for AIR documents.
- **[defs/](defs/)** — Built-in schemas, effects, caps, and modules.
- **[test-vectors/](test-vectors/)** — Canonicalization and schema test vectors.
