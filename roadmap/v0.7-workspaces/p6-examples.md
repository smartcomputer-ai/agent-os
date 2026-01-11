# P6: Examples (09-workspaces)

**Priority**: P2  
**Effort**: Medium  
**Risk if deferred**: Medium  
**Status**: Complete

## Goal

Add a ladder example that exercises workspace effects via plans and caps, seeds one or two
workspaces, and leaves a usable world that can be operated with `aos ws`.

## Requirements

- Use plan orchestration (no reducer tree ops).
- Use workspace cap + policy enforcement.
- Create 1-2 workspaces (alpha, beta) if missing.
- Exercise resolve, empty_root, write_bytes, list, read_ref, read_bytes, diff (optional remove).
- Record commits in `sys/Workspace@1`.
- Example must run from `aos-examples` and from `aos run`.

## Design

### Assets and wiring

- New example root: `examples/09-workspaces/`
- AIR assets:
  - `air/manifest.air.json`
  - `air/schemas.air.json`
  - `air/workspace_plan.air.json`
  - `air/policies.air.json`
  - `air/capabilities.air.json`
- Reducer crate: `examples/09-workspaces/reducer`
- Runner: `crates/aos-examples/src/workspaces.rs`
- Update `crates/aos-examples/src/main.rs` + `examples/README.md` to add "09-workspaces".

### Manifest routing

- `demo/WorkspaceEvent@1` -> `demo/WorkspaceDemo@1`
- `sys/WorkspaceCommit@1` -> `sys/Workspace@1` (key_field: "workspace")
- Trigger `demo/workspace_seed_plan@1` on `demo/WorkspaceSeed@1`
- Include all workspace schemas/effects used.
- Default policy + cap grants for workspace plan.

### Schemas

Define minimal demo schemas:
- `demo/WorkspaceEvent@1` (variant)
  - `Start { workspaces: [text], owner: text }`
  - `Seeded { workspace: text, expected_head: option<nat>, root_hash: hash, entry_count: nat, diff_count: nat, owner: text }`
- `demo/WorkspaceSeed@1` (record)
  - `workspace: text`, `owner: text`
- `demo/WorkspaceState@1` (record)
  - `workspaces: map<text, demo/WorkspaceSummary@1>`
- `demo/WorkspaceSummary@1` (record)
  - `version: option<nat>`, `root_hash: hash`, `entry_count: nat`, `diff_count: nat`

### Reducer behavior (demo/WorkspaceDemo@1)

- On `Start`: emit `demo/WorkspaceSeed@1` for each workspace name with owner from event.
- On `Seeded`:
  - update state summary for the workspace.
  - emit `sys/WorkspaceCommit@1` with `workspace`, `expected_head`, and
    `meta { root_hash, owner, created_at: ctx.now_ns }`.

### Plan behavior (demo/workspace_seed_plan@1)

Plan input: `demo/WorkspaceSeed@1`. Use guard edges on `resolve.exists`.

Steps:
1) `workspace.resolve` -> `resolve`
2) If `resolve.exists`:
   - `assign base_root = resolve.root_hash`
3) Else:
   - `workspace.empty_root` -> `empty_root`
   - `assign base_root = empty_root.root_hash`
4) `workspace.write_bytes` README.txt -> `root_a`
5) `workspace.write_bytes` data.json -> `root_b`
6) `workspace.list` (scope subtree) -> `entries`
7) `workspace.read_ref` + `workspace.read_bytes` README.txt
8) `workspace.write_bytes` update README.txt -> `root_c`
9) `workspace.diff` root_b vs root_c -> `changes`
10) `raise_event` `demo/WorkspaceEvent@1::Seeded` with
    `workspace`, `expected_head = resolve.resolved_version`,
    `root_hash = root_c`, `entry_count = len(entries)`,
    `diff_count = len(changes)`, `owner = @plan.input.owner`
11) `end`

Plan metadata:
- `required_caps`: `cap_workspace`
- `allowed_effects`: workspace.resolve, workspace.empty_root, workspace.write_bytes,
  workspace.list, workspace.read_ref, workspace.read_bytes, workspace.diff

### Caps and policy

- `demo/workspace_cap@1` uses `sys/workspace@1` with:
  - `workspaces: ["alpha", "beta"]` (narrow scope)
  - `ops: ["resolve","empty_root","write_bytes","list","read_ref","read_bytes","diff"]`
- `demo/workspace-policy@1` allows only those effects from `demo/workspace_seed_plan@1`,
  deny all else.

### Runner behavior

- `aos-examples workspaces`:
  - compile demo reducer
  - send `demo/WorkspaceEvent@1::Start` with workspaces ["alpha","beta"]
  - run to idle (all effects are internal)
  - print summary + verify replay
- Keep the example world on disk for CLI use.

### CLI interaction

- After seeding:
  - `AOS_WORLD=examples/09-workspaces aos ws ls`
  - `AOS_WORLD=examples/09-workspaces aos ws cat alpha/README.txt`
- Or run as daemon:
  - `aos run --world examples/09-workspaces`
  - `aos ws write alpha/new.txt --text "hi"`
- Plan-triggered add:
  - `aos event send demo/WorkspaceSeed@1 '{"workspace":"gamma","owner":"demo"}'`

## Implementation checklist

- [x] Add `examples/09-workspaces` assets and reducer crate.
- [x] Add runner + CLI wiring (main.rs, README).
- [x] Extend `crates/aos-examples/src/example_host.rs` to patch `sys/Workspace@1` wasm.
- [ ] Add any minimal tests if needed (optional).
