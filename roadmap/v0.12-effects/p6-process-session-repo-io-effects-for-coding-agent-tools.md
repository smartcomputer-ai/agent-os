# P6: Process-Session Repo I/O Effects for Coding Agent Tools

**Priority**: P6  
**Status**: Proposed  
**Date**: 2026-02-28

## Goal

Enable coding-agent tool profiles (OpenAI/Anthropic/Gemini aligned) to operate on
real Unix repositories under the `process.session` security boundary.

This slice adds external repo file/search/edit effects scoped to an opened
process session, so agents can reliably do:

1. `read_file`
2. `write_file`
3. `edit_file`
4. `apply_patch`
5. `grep`
6. `glob`
7. `shell` (already covered by `process.exec`)

## Clarification (Important)

`workspace.*` effects are AOS internal world storage primitives. They are not the
right surface for editing host checkout files in a Unix repo.

For coding-agent "real repo" workflows, effects must operate on host filesystem
paths inside the sandbox/session selected by `process.session.open`.

## Why This Belongs Under `process.*`

P4 defines `process.session.open` as the primary security boundary.

Putting repo I/O effects under `process.*` keeps all host-side authority
session-scoped:

1. same `session_id`,
2. same mounts/workdir/env/network policy,
3. same cap/policy posture,
4. same adapter routing model.

## Proposed Effect Family

Keep existing P4 effects:

1. `process.session.open`
2. `process.exec`
3. `process.session.signal`

Add new session-scoped repo effects:

1. `process.fs.read_file`
2. `process.fs.write_file`
3. `process.fs.edit_file`
4. `process.fs.apply_patch`
5. `process.fs.grep`
6. `process.fs.glob`

Optional but recommended:

1. `process.fs.stat`
2. `process.fs.exists`
3. `process.fs.list_dir`

## Tool Mapping (Factory + External Loop Spec)

1. `read_file` -> `process.fs.read_file`
2. `write_file` -> `process.fs.write_file`
3. `edit_file` -> `process.fs.edit_file`
4. `apply_patch` -> `process.fs.apply_patch`
5. `grep` -> `process.fs.grep`
6. `glob` -> `process.fs.glob`
7. `shell` -> `process.exec`

Subagent tools do not require new external effects in this slice:

1. `spawn_agent`
2. `send_input`
3. `wait`
4. `close_agent`

Those are orchestration/runtime concerns in session state + host loop.

## Minimal Contract Shape (v0.12)

All new `process.fs.*` params include `session_id`.

### `process.fs.read_file`

Params:

1. `session_id`
2. `path` (session-relative or canonicalized under allowed roots)
3. `offset?` (line or byte offset; choose one mode for v0.12 and keep stable)
4. `limit?`
5. `encoding?` (`utf8|bytes`)

Receipt:

1. `content` variant: `inline_text | inline_bytes | blob`
2. `truncated` bool
3. `size_bytes`

### `process.fs.write_file`

Params:

1. `session_id`
2. `path`
3. `content` variant: `text | bytes | blob_ref`
4. `create_parents?`
5. `mode?` (`overwrite|create_new`)

Receipt:

1. `written_bytes`
2. `created` bool
3. `new_mtime_ns?`

### `process.fs.edit_file`

Params:

1. `session_id`
2. `path`
3. `old_string`
4. `new_string`
5. `replace_all?`

Receipt:

1. `replacements`
2. `applied` bool
3. `error_code?` (for ambiguous/not-found semantics)

### `process.fs.apply_patch`

Params:

1. `session_id`
2. `patch_text` (v4a-compatible in v0.12)
3. `dry_run?`

Receipt:

1. `files_changed`
2. `ops` summary (add/update/delete/move counts)
3. `errors?`

### `process.fs.grep`

Params:

1. `session_id`
2. `pattern`
3. `path?`
4. `glob_filter?`
5. `case_insensitive?`
6. `max_results?`

Receipt:

1. `matches` variant: `inline_text | blob`
2. `match_count`
3. `truncated` bool

### `process.fs.glob`

Params:

1. `session_id`
2. `pattern`
3. `path?`
4. `max_results?`

Receipt:

1. `paths` variant: `inline_text | blob`
2. `count`
3. `truncated` bool

## Capability and Policy

Extend process capability schema/enforcer to include fs constraints:

1. `allowed_fs_ops` (read/write/edit/patch/search/list/stat)
2. `fs_roots` (path prefixes)
3. `max_read_bytes`
4. `max_write_bytes`
5. `max_patch_bytes`
6. `follow_symlinks` policy (`deny|within_root_only|allow`)

Enforcer rules:

1. fail closed on path escaping (including symlink traversal outside roots),
2. validate requested op is allowed,
3. enforce size limits before expensive operations.

## Adapter Behavior

Implement new in-process adapters that share process session state:

1. resolve session and validate active/not expired,
2. canonicalize path against session workdir + allowed roots,
3. perform operation with deterministic receipt encoding,
4. return large payloads as blob refs (consistent with process output + P5 posture).

Implementation guidance:

1. `apply_patch` should be host-native patch parsing/apply, not shelling out to `patch`.
2. `grep` can use `rg` when available with deterministic fallback.
3. `glob` should be host-native glob/walk with deterministic sorting.

## Determinism and Replay

1. External filesystem/process actions are never replayed.
2. Replay consumes journaled receipts/events exactly as with other effects.
3. Large receipt payloads should use CAS refs so replay dependencies remain explicit.
4. Missing CAS dependency during replay is deterministic hard fault.

## Rollout Plan

### Phase 6.1: Contracts + Capability

1. Add `defeffect` + schemas for `process.fs.*` minimal set.
2. Extend `sys/ProcessCapParams@1` and `sys/CapEnforceProcess@1` with fs constraints.
3. Wire built-ins and validation.

### Phase 6.2: Host Adapters

1. Add adapters for `process.fs.read_file/write_file/edit_file/apply_patch/grep/glob`.
2. Share session registry and route defaults.
3. Enforce path canonicalization and symlink policy.

### Phase 6.3: Agent SDK Integration

1. Extend SDK tool catalog/runtime mapping to use `process.fs.*` + `process.exec`.
2. Add provider-aligned profile mapping:
   - OpenAI: prefer `apply_patch`
   - Anthropic/Gemini: prefer `edit_file`
3. Keep existing session/tool-batch state machine semantics.

### Phase 6.4: Verification

1. Integration tests for each new effect (success + deny + bounds).
2. E2E coding-agent fixture on a real checked-out repo:
   - read/edit/write/patch
   - grep/glob discovery
   - shell test run
3. Replay-or-die coverage with snapshots/journal tail.

## Risks

1. Path traversal/symlink escapes if canonicalization is incomplete.
2. Non-deterministic grep/glob ordering across platforms.
3. Excessively large inline receipts causing journal/context bloat.
4. Semantic drift between provider-native tool expectations and effect contracts.

## Non-Goals

1. Replacing `process.exec` shell workflows.
2. Moving host repo operations into `workspace.*`.
3. Shipping remote worker execution in this slice.
4. Implementing PR/GitHub API effects (use `process.exec` + existing adapters for now).

## Deliverables / DoD

1. Process-session-scoped repo effects exist for core coding tools.
2. `shell` + file/search/edit tools run under one `session_id` authority boundary.
3. Cap/policy can constrain repo roots and fs operations deterministically.
4. SDK tool runtime can execute factory core coding flows end-to-end on real repos.
5. Integration + e2e + replay tests pass.
