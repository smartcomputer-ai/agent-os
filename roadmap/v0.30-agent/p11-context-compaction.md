# P11: Context Compaction and Token Budgeting

**Priority**: P1
**Effort**: Large
**Risk if deferred**: High (long-running sessions will eventually overflow provider windows, and compaction decisions will be hard to audit if hidden inside provider adapters)
**Status**: Proposed
**Depends on**: `roadmap/v0.30-agent/p5-session-run-model.md`, `roadmap/v0.30-agent/p6-turn-planner.md`, `roadmap/v0.30-agent/p7-run-traces-and-intervention.md`

## Goal

Add explicit, deterministic SDK support for token counting and context compaction.

Primary outcome:

1. the turn planner can make budget decisions from provider-accurate token counts,
2. compaction is an auditable workflow decision, not hidden adapter behavior,
3. full session history remains durable and replayable,
4. the active model window can be replaced by compacted summaries or provider-native compaction artifacts,
5. OpenAI, Anthropic, and generic model backends can share the same SDK-level control flow while using different provider capabilities underneath.

## Problem

The current session model keeps transcript refs and turn-planner inputs close together. That works for short sessions, but it does not scale:

1. transcript history grows without bound,
2. `TurnBudget` exists but currently does not have reliable per-turn token counts,
3. provider response usage tells us what just happened but not whether the next request will fit,
4. local tokenizer estimates are useful but not authoritative for tools, structured output, images, files, provider metadata, and model-specific rendering,
5. provider-specific compaction APIs can produce opaque artifacts that are not portable across providers,
6. if compaction happens inside `sys/llm.generate@1`, session state can silently diverge from what the planner and trace claim happened.

The SDK needs a durable split between:

1. full audit history, and
2. the active context window sent to the next model call.

## Provider Fit

### OpenAI

OpenAI Responses supports provider-native token counting and compaction.

Relevant capabilities:

1. token counting for Responses-style inputs,
2. server-side compaction from a Responses create request through context-management options,
3. standalone compaction through a Responses compaction endpoint,
4. opaque encrypted compaction items that can carry prior context forward using fewer tokens.

SDK implication:

1. OpenAI token counting is a good backend for `sys/llm.count_tokens@1`.
2. OpenAI standalone compaction is a good backend for `sys/llm.compact@1`.
3. OpenAI server-side compaction may be enabled through `sys/llm.generate@1` provider options, but any returned compaction item must be surfaced in the receipt and trace.
4. OpenAI compaction artifacts are provider-native inputs, not generic AOS summaries.

### Anthropic

Anthropic has token counting and context editing. Context editing is not the same thing as durable SDK compaction.

Relevant capabilities:

1. provider token counting for Messages-style requests,
2. context-editing options that can clear older tool results and related context during request processing,
3. client-side history remains the durable source of truth.

SDK implication:

1. Anthropic token counting is a good backend for `sys/llm.count_tokens@1`.
2. Anthropic context editing should be modeled as provider request shaping on `sys/llm.generate@1` or `sys/llm.count_tokens@1`, with applied edits recorded in receipt metadata.
3. Durable SDK compaction for Anthropic should still use `sys/llm.compact@1`, usually backed by AOS-owned summarization.

### Generic Providers

Most providers will not have durable compaction APIs.

SDK implication:

1. `sys/llm.count_tokens@1` can use provider token-count APIs when available, a provider tokenizer when available, or a marked estimate otherwise.
2. `sys/llm.compact@1` should support AOS-owned summarization through an LLM call or a deterministic summarizer when available.
3. compaction receipts must say whether the result is provider-native, AOS-summary, local-estimate-derived, or unavailable.

## Design Stance

### 1) Add explicit LLM effects

Add two effects:

1. `sys/llm.count_tokens@1`
2. `sys/llm.compact@1`

Do not overload `sys/llm.generate@1` as the only context-management boundary.

`sys/llm.generate@1` can still expose provider-native context-management options, but those options must remain request-shaping options unless their results are explicitly surfaced in receipts and applied by the workflow.

### 2) Token counting is a preflight effect

`sys/llm.count_tokens@1` is non-mutating.

It accepts a provider/model plus refs for the same rendered inputs the planner wants to send to `sys/llm.generate@1`.

The receipt should return:

1. total input tokens,
2. optional per-lane or per-ref counts when the provider/backend can support them,
3. tool/schema token contribution when available,
4. provider/model id used for counting,
5. count quality: `Exact`, `ProviderEstimate`, `LocalEstimate`, or `Unknown`,
6. provider metadata for diagnostics.

The planner should use this to decide whether to proceed, drop lower-priority inputs, or compact.

### 3) Compaction is a workflow state transition

`sys/llm.compact@1` is an explicit effect because compaction changes the active model window.

It accepts:

1. provider/model and compaction strategy,
2. source window refs or a range in the full transcript ledger,
3. preserve refs that must not be summarized away,
4. recent-tail policy,
5. target token budget,
6. optional provider-specific parameters.

The receipt should return:

1. compaction artifact refs,
2. artifact kind: `AosSummary`, `ProviderNative`, or `Mixed`,
3. compacted source range,
4. compacted-through marker,
5. input/output token usage when available,
6. provider metadata,
7. warnings when the result is non-portable or approximate.

The workflow, not the adapter, applies the receipt to session state.

### 4) Keep full history separate from the active model window

Add an explicit state split:

1. transcript ledger: append-only, durable, auditable full session history,
2. active window: refs eligible for the next model request,
3. compaction records: append-only records of compacted ranges and artifacts,
4. compacted-through marker: source history position represented by compaction artifacts,
5. recent tail: un-compacted recent message refs kept verbatim.

Old transcript blobs are never deleted as part of compaction.

### 5) The turn planner owns compaction triggers

Compaction should be triggered from turn planning.

Recommended planner flow:

1. build candidate turn plan from pinned, durable, runtime, skill, memory, and user inputs,
2. if token estimates are missing or stale, return a `CountTokens` prerequisite,
3. after token counts return, re-plan with concrete counts,
4. if over soft budget, drop optional low-priority inputs first,
5. if still over budget, return a `CompactContext` prerequisite,
6. after compaction receipt updates active window state, re-plan,
7. dispatch `sys/llm.generate@1` only when the active turn fits hard budget.

This keeps compaction observable through `TurnPlanned` and run trace entries.

### 6) Provider-native compaction is not portable memory

Provider-native compaction artifacts are valid active-window inputs only for the compatible provider/model family.

They should not be treated as:

1. human-readable summaries,
2. cross-provider memory,
3. source-of-truth history,
4. durable replacement for transcript refs.

When provider-native artifacts exist, the planner must know their provider compatibility before selecting them for a turn.

### 7) AOS summaries are portable but lossy

AOS-owned summaries are normal AOS blobs/refs. They are portable across providers, inspectable, and suitable for replay.

They are also lossy. Each summary needs:

1. source range,
2. summary prompt/version,
3. model/provider used to produce it,
4. token usage,
5. creation trace,
6. optional quality warnings.

Do not pretend summaries preserve the exact original conversation. Preserve originals separately in the transcript ledger.

## Proposed Contracts

### `LlmCountTokensParams`

Fields:

1. `provider`: provider id,
2. `model`: model id,
3. `message_refs`: active message/input refs,
4. `tool_definitions_ref`: optional tool definitions blob ref,
5. `response_format_ref`: optional response format ref,
6. `provider_options_ref`: optional provider options ref,
7. `rendering_profile`: optional provider/API rendering mode,
8. `metadata`: freeform request metadata.

### `LlmCountTokensReceipt`

Fields:

1. `input_tokens`: optional integer,
2. `counts_by_ref`: optional list of `{ ref, tokens, quality }`,
3. `tool_tokens`: optional integer,
4. `response_format_tokens`: optional integer,
5. `quality`: `Exact | ProviderEstimate | LocalEstimate | Unknown`,
6. `provider`: provider id,
7. `model`: model id,
8. `provider_metadata`: optional CBOR/JSON value,
9. `warnings`: list of strings or typed warning codes.

### `LlmCompactParams`

Fields:

1. `provider`: provider id,
2. `model`: model id,
3. `strategy`: `ProviderNative | AosSummary | Auto`,
4. `source_range`: compactable transcript ledger range,
5. `source_refs`: optional explicit refs,
6. `preserve_refs`: refs that must remain verbatim,
7. `recent_tail_count`: minimum recent messages to keep verbatim,
8. `target_input_tokens`: optional target budget,
9. `summary_prompt_ref`: optional prompt ref for AOS summary strategy,
10. `provider_options_ref`: optional provider-specific options,
11. `metadata`: freeform request metadata.

### `LlmCompactReceipt`

Fields:

1. `artifact_refs`: refs to summary/provider-native artifacts,
2. `artifact_kind`: `AosSummary | ProviderNative | Mixed`,
3. `compacted_range`: transcript ledger range represented by the artifacts,
4. `compacted_through`: transcript ledger sequence marker,
5. `active_window_refs`: recommended replacement refs for the compacted portion,
6. `usage`: optional usage record,
7. `provider`: provider id,
8. `model`: model id,
9. `provider_metadata`: optional CBOR/JSON value,
10. `warnings`: list of strings or typed warning codes.

## Proposed State

Add or evolve session state toward:

1. `transcript_ledger_ref`: durable/chunked append-only transcript ledger,
2. `active_window_refs`: ordered refs selected as the base window for the next turn,
3. `compaction_records`: append-only compaction history,
4. `compacted_through`: optional transcript ledger sequence marker,
5. `last_token_count`: optional latest count result for diagnostics,
6. `last_compaction`: optional latest compaction record for diagnostics.

Existing `transcript_message_refs` can be migrated into the ledger/window split. Because this SDK is still experimental, prefer the clean split over compatibility shims.

## Proposed Planner Prerequisites

Extend turn planning prerequisites with:

1. `CountTokens`
2. `CompactContext`

`CountTokens` should include the candidate plan identity so stale token-count receipts cannot be applied to a different plan.

`CompactContext` should include:

1. source range,
2. chosen strategy,
3. budget target,
4. candidate plan identity,
5. reason: `OverSoftBudget`, `OverHardBudget`, `Manual`, or `ProviderRequired`.

## Trace Events

Add trace coverage for:

1. `TokenCountRequested`,
2. `TokenCountReceived`,
3. `CompactionRequested`,
4. `CompactionReceived`,
5. `ActiveWindowUpdated`.

`TurnPlanned` should continue to show selected input refs and token budget status.

Compaction traces should include refs and metadata, not inline prompt/history text.

## First Cut

Implement the narrowest useful version:

1. add contracts for `sys/llm.count_tokens@1`,
2. add contracts for `sys/llm.compact@1`,
3. add `CountTokens` and `CompactContext` planner prerequisites,
4. add state fields for `active_window_refs`, `compaction_records`, and latest token/compaction diagnostics,
5. make the workflow able to request token counts before LLM dispatch,
6. make the workflow able to apply a scripted compaction receipt,
7. add deterministic harness tests that script count and compaction receipts,
8. keep provider adapter implementation minimal or stubbed until the planner/state path is proven.

Do not implement a full summarizer, memory engine, provider compatibility matrix, or UI in the first cut.

## Deferred

1. full AOS summary prompt design,
2. automatic summary quality checks,
3. cross-provider summary migration,
4. memory/RAG integration,
5. deletion GC for old transcript blobs,
6. UI/operator controls for manual compaction,
7. provider-specific optimization for prompt caching,
8. live evals for compaction quality,
9. per-message token attribution when providers cannot supply it,
10. sophisticated rolling-window policies beyond summary plus recent tail.

## Acceptance Criteria

1. [ ] token counting is represented as `sys/llm.count_tokens@1`, not ad hoc provider logic inside the workflow,
2. [ ] compaction is represented as `sys/llm.compact@1`, not hidden mutation inside `sys/llm.generate@1`,
3. [ ] turn planning can request token counting before generation,
4. [ ] turn planning can request compaction before generation,
5. [ ] full transcript history remains durable after compaction,
6. [ ] active model window can be replaced by compaction artifacts plus recent tail,
7. [ ] compaction records are append-only and traceable,
8. [ ] provider-native compaction artifacts are marked provider-specific,
9. [ ] AOS summaries are represented as normal refs with source ranges and usage metadata,
10. [ ] deterministic tests can script token-count and compaction receipts without live provider credentials.

