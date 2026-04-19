# AgentOS Specs

This directory is the source-of-truth shelf for active AgentOS protocol and runtime specs.

## Index

Active specs:

- **01 — [Overview](01-overview.md)**: core concepts and mental model.
- **02 — [Architecture](02-architecture.md)**: runtime components, storage layout, execution, and governance.
- **03 — [AIR](03-air.md)**: AIR v1 control-plane IR, schemas, manifests, effects, capabilities, policies, and patch format.
- **04 — [Workflows](04-workflows.md)**: workflow module runtime contract, orchestration patterns, and keyed cells.
- **05 — [Effects](05-effects.md)**: async execution, durable open work, adapters, receipts, and continuation admission.

Future protocol notes:

- **20 — [GC and Reachability](20-gc.md)**: draft reachability and future GC contract. Mark/sweep deletion is not implemented yet.

## Reference Shelves

- **[schemas/](schemas/)** — JSON Schemas for AIR documents.
- **[defs/](defs/)** — Built-in schemas, effects, caps, and modules.
- **[test-vectors/](test-vectors/)** — Canonicalization and schema test vectors.
