# P6: Host-Session Repo I/O Effects for Coding Agent Tools

**Priority**: P6  
**Status**: Complete  
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
4. Keep payload variant conventions aligned with P4 `host.exec` where
   applicable:
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
2. `patch` (required, non-empty) variant:
   - `inline_text { text }` (SDK tool arg `patch` maps here)
   - `blob_ref { blob_ref }` (optional path for very large/non-LLM patch input)
3. `patch_format?` (optional, default `v4a`)
4. `dry_run?` (optional, default `false`)

Receipt:

1. `status`: `ok|parse_error|reject|not_found|forbidden|error`
2. `files_changed?`
3. `changed_paths?` (ordered list of affected paths)
4. `ops?` summary (add/update/delete/move counts)
5. `summary_text?` (tool UX, e.g. `Applied patch: ...`)
6. `errors?` (bounded diagnostics)
7. `error_code?`

Behavior contract:

1. Adapter delegates parsing + apply semantics to the shared patch library.
2. v0.12 does not re-specify full patch grammar here; accepted patch syntax is
   the syntax accepted by the selected `patch_format` implementation.
3. `dry_run=true` validates/applies in memory and returns would-change metadata
   without committing writes.
4. On parse/verification failure, return non-`ok` status with machine-readable
   `error_code`; do not emit partial writes.
5. SDK/tool layer may render `summary_text` as provider-facing success text.

### `host.fs.grep`

Params:

1. `session_id`
2. `pattern` (required regex pattern)
3. `path?` (file or directory; default: session working dir)
4. `glob_filter?` (optional file filter, e.g. `*.rs`)
5. `case_insensitive?` (optional, default `false`)
6. `max_results?` (optional, default `100`)
7. `output_mode?` (`auto|require_inline`)

Receipt:

1. `status`: `ok|invalid_regex|not_found|forbidden|error`
2. `matches?` variant:
   - `inline_text { text }`
   - `blob { blob_ref, size_bytes, preview_bytes? }`
3. `match_count?`
4. `truncated?` bool
5. `error_code?`
6. `summary_text?` (optional tool UX for empty/non-empty summary strings)

Behavior contract:

1. Primary backend is `ripgrep` when available; deterministic native regex
   fallback is required when `ripgrep` is unavailable.
2. Successful output represents matching lines with file paths + line numbers.
3. Empty match set is represented as `status: ok` with `match_count: 0`; SDK/tool
   adapters may map this to provider-facing text such as `No matches found`.
4. Results must be deterministic for identical inputs on the same host/session.

### `host.fs.glob`

Params:

1. `session_id`
2. `pattern` (required glob pattern, e.g. `**/*.ts`)
3. `path?` (base directory; default: session working dir)
4. `max_results?` (optional; adapter default applies if omitted)
5. `output_mode?` (`auto|require_inline`)

Receipt:

1. `status`: `ok|invalid_pattern|not_found|forbidden|error`
2. `paths?` variant:
   - `inline_text { text }`
   - `blob { blob_ref, size_bytes, preview_bytes? }`
3. `count?`
4. `truncated?` bool
5. `error_code?`
6. `summary_text?` (optional tool UX for empty/non-empty summary strings)

Behavior contract:

1. Uses host-native filesystem globbing/walk.
2. Result ordering is deterministic and sorted by mtime descending (newest
   first), with stable path tie-break for equal mtimes.
3. Empty match set is represented as `status: ok` with `count: 0`; SDK/tool
   adapters may map this to provider-facing text such as `No files matched`.

## Capability and Policy

Extend host capability schema/enforcer to include fs constraints:

1. `allowed_fs_ops` (read/write/edit/patch/search/list/stat)
2. `fs_roots` (path prefixes)
3. `max_read_bytes?`
4. `max_write_bytes?`
5. `max_patch_bytes?`
6. `max_inline_bytes?` (upper bound for inline receipt payloads)
7. `max_grep_results?`
8. `max_glob_results?`
9. `max_scan_files?`
10. `max_scan_bytes?`
11. `allowed_patch_formats?` (v0.12 default/required value when set: `v4a`)
12. `max_changed_files?` (patch blast-radius limit)
13. `max_edit_replacements?`
14. `follow_symlinks` policy (`deny|within_root_only|allow`)

Limit fields are intentionally optional in v0.12 for minimal friction. When a
limit is omitted, that dimension is unbounded by capability policy (subject only
to adapter/runtime safety behavior).

Enforcer rules:

1. fail closed on path escaping (including symlink traversal outside roots),
2. validate requested op is allowed,
3. enforce size/count limits only when configured,
4. when a limit is configured, clamp request-level limits to cap ceilings
   (`effective = min(request, cap)`),
5. for `output_mode=require_inline`, fail deterministically when payload would
   exceed `max_inline_bytes` (when configured).

Policy interaction:

1. Policy may further restrict/deny operations even when capability allows them.
2. Policy may enforce narrower path scopes per op (for example allow `grep` on
   repo root but deny `write_file` outside `src/`).
3. Policy may enforce stricter output posture (for example force blob-only on
   selected paths/content classes).

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

1. [x] Add `defeffect` + schemas for `host.fs.*` minimal set.
2. [x] Extend `sys/HostCapParams@1` and `sys/CapEnforceHost@1` with fs constraints.
3. [x] Wire built-ins and validation.

### Phase 6.2: Host Adapters

1. [x] Add adapters for `host.fs.read_file/write_file/edit_file/apply_patch/grep/glob`.
2. [x] Share session registry and route defaults.
3. [x] Enforce path canonicalization and symlink policy.

### Phase 6.3: Agent SDK Integration

1. [x] Extend SDK tool catalog/runtime mapping to use `host.fs.*` + `host.exec`.
2. [x] Add provider-aligned profile mapping:
   - OpenAI: prefer `apply_patch`
   - Anthropic/Gemini: prefer `edit_file`
3. [x] Keep existing session/tool-batch state machine semantics.

### Phase 6.4: Verification

1. [x] Add integration tests under `crates/aos-host/tests` for each effect:
   - `host.fs.read_file`: `inline_text`, `inline_bytes`, `blob`, `require_inline`
     overflow behavior, not-found/forbidden.
   - `host.fs.write_file`: `inline_text`, `inline_bytes`, `blob_ref`, mode
     semantics (`overwrite|create_new`), conflict/forbidden.
   - `host.fs.edit_file`: exact match, fuzzy match fallback, ambiguous error when
     `replace_all=false`, replace-all semantics, empty `old_string` error.
   - `host.fs.apply_patch`: success, parse error, reject/not-found, `dry_run`
     no-write behavior, no partial-write on failure.
   - `host.fs.grep`: rg path + native fallback path, invalid regex, not-found,
     empty result (`match_count=0`), output truncation/inline-vs-blob.
   - `host.fs.glob`: invalid pattern, not-found, deterministic ordering, empty
     result (`count=0`), inline-vs-blob.
2. [x] Add e2e tests under `crates/aos-host/tests_e2e` on a real checkout/session:
   - open one `host.session`, run read/edit/write/apply_patch/grep/glob/exec in
     one flow.
   - assert cap/policy denials are enforced for out-of-root or disallowed ops.
   - assert large outputs use blob refs under `auto` and error deterministically
     under `require_inline` when configured limits are exceeded.
3. [x] Add replay-or-die coverage in kernel/host test flows:
   - execute run, persist journal/snapshot, replay from genesis, assert
     byte-identical snapshot.
   - ensure CAS refs referenced by receipts remain reachable from
     snapshot+journal.
4. [x] Add limit-configuration tests (when limits are set) and permissive baseline
   tests (when limits are omitted) to validate optional-limit behavior.

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

## Completion Notes (2026-03-01)

1. Added and wired built-in effect kinds, schemas, and capability constraints for
   `host.fs.read_file`, `host.fs.write_file`, `host.fs.edit_file`,
   `host.fs.apply_patch`, `host.fs.grep`, `host.fs.glob`, plus
   `host.fs.stat`, `host.fs.exists`, and `host.fs.list_dir`.
2. Split host-related schemas into `spec/defs/builtin-schemas-host.air.json`
   and merged host/core schema loading in built-in schema resolution.
3. Refactored host adapter internals into `crates/aos-host/src/adapters/host/`
   with isolated modules for path resolution, output materialization, session
   state, and patch parse/apply/edit internals.
4. Registered host fs adapters in default host routing and adapter registry so
   host session + repo I/O tools run under a shared `session_id` boundary.
5. Added focused host fs integration coverage in
   `crates/aos-host/tests/adapters_host_fs_integration.rs` covering:
   read/write/edit/apply_patch/grep/glob/stat/exists/list_dir status paths,
   output-mode behavior (`auto` inline/blob and `require_inline` errors), and
   deterministic/atomic behavior expectations.
6. Added end-to-end host fs flow coverage in
   `crates/aos-host/tests_e2e/host_fs_adapter_e2e.rs` and wired test target in
   `crates/aos-host/Cargo.toml`.
7. Added host capability-enforcer denial coverage in
   `crates/aos-host/tests_e2e/cap_enforcer_e2e.rs`
   (`host_enforcer_module_denies_disallowed_fs_op`) to verify disallowed
   host-fs operations are rejected through `sys/CapEnforceHost@1`.
