# 09-workspaces

Demonstrates workspace effects via a plan with capability enforcement. Seeds one or two
workspaces (alpha, beta), writes files, lists and diffs the tree, and emits commit events.

Plan flow:
```mermaid
flowchart TD
  A[WorkspaceSeed event] --> B[workspace.resolve]
  B --> C{exists?}
  C -->|yes| D[use resolved root]
  C -->|no| E[workspace.empty_root]
  D --> F[write seed/<workspace>.txt]
  E --> F
  F --> G[write README.txt]
  G --> H[write data.json]
  H --> I[workspace.list - subtree]
  I --> J[read README ref/bytes]
  J --> K[update README.txt]
  K --> L[workspace.diff]
  L --> M[raise Seeded event]
  M --> N[reducer updates state + emits WorkspaceCommit]
```

Run:
- `cargo run -p aos-examples -- workspaces`
- `AOS_WORLD=examples/09-workspaces aos ws ls`

Note:
- The plan writes a per-workspace marker file under `seed/` before other writes so effect intents
  remain unique when seeding multiple workspaces in the same world. The reducer also treats
  identical `Seeded` events as idempotent to keep replay deterministic.
- Each `emit_effect` sets `idempotency_key` via `hash({workspace, step})` so retries are stable
  and intent hashes stay unique across parallel workspace seeds.
