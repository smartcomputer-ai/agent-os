# AgentOS Specs

This directory is the source-of-truth shelf for active AgentOS protocol and runtime specs.

## Index

Active specs:

- **01 — [Overview](01-overview.md)**: core concepts and mental model.
- **02 — [Architecture](02-architecture.md)**: runtime components, storage layout, execution, and governance.
- **03 — [AIR](03-air.md)**: AIR v2 control-plane IR, schemas, manifests, ops, routing, secrets, receipts, and patch format.
- **04 — [Workflows](04-workflows.md)**: workflow op runtime contract, orchestration patterns, and keyed cells.
- **05 — [Effects](05-effects.md)**: async execution, durable open work, adapters, receipts, and continuation admission.
- **06 — [Backends](06-backends.md)**: SQLite/Kafka journals, local/object-store CAS, checkpoint metadata, and recovery.

Future protocol notes:

- **20 — [GC and Reachability](20-gc.md)**: draft reachability and future GC contract. Mark/sweep deletion is not implemented yet.

## Reference Shelves

- **[schemas/](schemas/)** — JSON Schemas for AIR documents.
- **[defs/](defs/)** — Built-in schemas, modules, and ops.
- **[test-vectors/](test-vectors/)** — Canonicalization and schema test vectors.
