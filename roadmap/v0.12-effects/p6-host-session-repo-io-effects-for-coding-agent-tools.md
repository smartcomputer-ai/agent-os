# P6: Host-Session Repo I/O Effects for Coding Agent Tools

**Priority**: P6  
**Status**: Proposed  
**Date**: 2026-02-28

## Goal

Enable coding-agent tool profiles (OpenAI/Anthropic/Gemini aligned) to operate on
real Unix repositories under the `host.session` security boundary.

This slice adds external repo file/search/edit effects scoped to an opened
host session, so agents can reliably do:

1. `read_file`
2. `write_file`
3. `edit_file`
4. `apply_patch`
5. `grep`
6. `glob`
7. `shell` (already covered by `host.exec`)

## Clarification (Important)

`workspace.*` effects are AOS internal world storage primitives. They are not the
right surface for editing host checkout files in a Unix repo.

For coding-agent "real repo" workflows, effects must operate on host filesystem
paths inside the sandbox/session selected by `host.session.open`.

## Why This Belongs Under `host.*`

P4 defines `host.session.open` as the primary security boundary.

Putting repo I/O effects under `host.*` keeps all host-side authority
session-scoped:

1. same `session_id`,
2. same mounts/workdir/env/network policy,
3. same cap/policy posture,
4. same adapter routing model.

## Proposed Effect Family

Keep existing P4 effects:

1. `host.session.open`
2. `host.exec`
3. `host.session.signal`

Add new session-scoped repo effects:

1. `host.fs.read_file`
2. `host.fs.write_file`
3. `host.fs.edit_file`
4. `host.fs.apply_patch`
5. `host.fs.grep`
6. `host.fs.glob`

Optional but recommended:

1. `host.fs.stat`
2. `host.fs.exists`
3. `host.fs.list_dir`

## Tool Mapping (Factory + External Loop Spec)

1. `read_file` -> `host.fs.read_file`
2. `write_file` -> `host.fs.write_file`
3. `edit_file` -> `host.fs.edit_file`
4. `apply_patch` -> `host.fs.apply_patch`
5. `grep` -> `host.fs.grep`
6. `glob` -> `host.fs.glob`
7. `shell` -> `host.exec`

Subagent tools do not require new external effects in this slice:

1. `spawn_agent`
2. `send_input`
3. `wait`
4. `close_agent`

Those are orchestration/runtime concerns in session state + host loop.

## Minimal Contract Shape (v0.12)

All new `host.fs.*` params include `session_id`.

Cross-effect alignment with P4:

1. For read/search/list style outputs, use `output_mode?: auto|require_inline`.
2. Under `auto`, adapters may return inline payloads for small data and `blob`
   payloads for large data.
3. Under `require_inline`, adapters must return inline payloads or fail with
   `status: error` and machine-readable `error_code` (for example
   `inline_required_too_large`).
4. Keep payload arms aligned with P4 `host.exec`:
   - `inline_text { text }`
   - `inline_bytes { bytes }`
   - `blob { blob_ref, size_bytes, preview_bytes? }`

LLM-facing default posture:

1. Default `output_mode` is `auto`.
2. For small UTF-8 payloads, prefer `inline_text` so tool results can be sent to
   the model without an extra blob fetch.
3. For large or non-inline-safe payloads, return `blob` deterministically.
4. Any future `host.fs.*` effect returning potentially large content should adopt
   this same `output_mode` contract.

### `host.fs.read_file`

Params:

1. `session_id`
2. `path` (session-relative or canonicalized under allowed roots)
3. `offset_bytes?`
4. `max_bytes?`
5. `encoding?` (`utf8|bytes`)
6. `output_mode?` (`auto|require_inline`)

Receipt:

1. `status`: `ok|not_found|is_directory|forbidden|error`
2. `content?` variant:
   - `inline_text { text }`
   - `inline_bytes { bytes }`
   - `blob { blob_ref, size_bytes, preview_bytes? }`
3. `truncated?` bool
4. `size_bytes?`
5. `error_code?`

### `host.fs.write_file`

Params:

1. `session_id`
2. `path`
3. `content` variant:
   - `inline_text { text }`
   - `inline_bytes { bytes }`
   - `blob_ref { blob_ref }`
4. `create_parents?`
5. `mode?` (`overwrite|create_new`)

Receipt:

1. `status`: `ok|conflict|forbidden|error`
2. `written_bytes?`
3. `created?` bool
4. `new_mtime_ns?`
5. `error_code?`

### `host.fs.edit_file`

Params:

1. `session_id`
2. `path` (SDK tool arg `file_path` maps here)
3. `old_string` (required, must be non-empty)
4. `new_string` (required)
5. `replace_all?` (optional, default `false`)

Receipt:

1. `status`: `ok|not_found|ambiguous|forbidden|error`
2. `replacements?`
3. `applied?` bool
4. `summary_text?` (for tool UX, e.g. `Updated <path> (<N> replacements)`)
5. `error_code?` (for ambiguous/not-found semantics)

Behavior contract (aligned in spirit with common coding-agent `edit_file` tools):

1. Read full file text, apply edit in-memory, then write full file back on
   success.
2. Match strategy is deterministic:
   - first attempt exact match search,
   - if no exact match is found, attempt deterministic fuzzy matching.
3. Fuzzy matching for v0.12 should include:
   - whitespace-normalized comparison,
   - quote/dash normalization equivalence.
4. If multiple candidate matches exist and `replace_all=false`, return
   `status: ambiguous` with a precise `error_code`.
5. If `replace_all=true`, replace all exact matches; if exact has zero matches,
   replace all accepted fuzzy matches.
6. If no match exists, return `status: not_found`.
7. If `old_string` is empty, return `status: error` with
   `error_code: invalid_input_empty_old_string`.
8. `replacements` is the number of applied replacements and MUST be stable for
   the same file content + params.
9. Adapter should use atomic write/rename semantics where possible to avoid
   partial writes.

Tool-facing adapter mapping guidance:

1. On success, SDK/tool layer may present `summary_text` as a plain success
   string for provider compatibility.
2. On failure, surface effect fault/error rather than fabricating a success
   payload.

### `host.fs.apply_patch`

Params:

1. `session_id`
2. `patch` variant:
   - `inline_text { text }`
   - `blob_ref { blob_ref }`
3. `dry_run?`

Receipt:

1. `status`: `ok|reject|forbidden|error`
2. `files_changed?`
3. `ops?` summary (add/update/delete/move counts)
4. `errors?`
5. `error_code?`

### `host.fs.grep`

Params:

1. `session_id`
2. `pattern`
3. `path?`
4. `glob_filter?`
5. `case_insensitive?`
6. `max_results?`
7. `output_mode?` (`auto|require_inline`)

Receipt:

1. `status`: `ok|forbidden|error`
2. `matches?` variant:
   - `inline_text { text }`
   - `blob { blob_ref, size_bytes, preview_bytes? }`
3. `match_count?`
4. `truncated?` bool
5. `error_code?`

### `host.fs.glob`

Params:

1. `session_id`
2. `pattern`
3. `path?`
4. `max_results?`
5. `output_mode?` (`auto|require_inline`)

Receipt:

1. `status`: `ok|forbidden|error`
2. `paths?` variant:
   - `inline_text { text }`
   - `blob { blob_ref, size_bytes, preview_bytes? }`
3. `count?`
4. `truncated?` bool
5. `error_code?`

## Capability and Policy

Extend host capability schema/enforcer to include fs constraints:

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

Implement new in-process adapters that share host session state:

1. resolve session and validate active/not expired,
2. canonicalize path against session workdir + allowed roots,
3. perform operation with deterministic receipt encoding,
4. follow P4 `output_mode` behavior: `auto` may inline-or-blob; `require_inline`
   must fail deterministically when limits would be exceeded.
5. return large payloads as blob refs (consistent with host output + P5 posture).

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

1. Add `defeffect` + schemas for `host.fs.*` minimal set.
2. Extend `sys/HostCapParams@1` and `sys/CapEnforceHost@1` with fs constraints.
3. Wire built-ins and validation.

### Phase 6.2: Host Adapters

1. Add adapters for `host.fs.read_file/write_file/edit_file/apply_patch/grep/glob`.
2. Share session registry and route defaults.
3. Enforce path canonicalization and symlink policy.

### Phase 6.3: Agent SDK Integration

1. Extend SDK tool catalog/runtime mapping to use `host.fs.*` + `host.exec`.
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

1. Replacing `host.exec` shell workflows.
2. Moving host repo operations into `workspace.*`.
3. Shipping remote worker execution in this slice.
4. Implementing PR/GitHub API effects (use `host.exec` + existing adapters for now).

## Deliverables / DoD

1. Host-session-scoped repo effects exist for core coding tools.
2. `shell` + file/search/edit tools run under one `session_id` authority boundary.
3. Cap/policy can constrain repo roots and fs operations deterministically.
4. SDK tool runtime can execute factory core coding flows end-to-end on real repos.
5. Integration + e2e + replay tests pass.
