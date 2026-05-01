# P11: Context Compaction

**Priority**: P1
**Effort**: Large
**Risk if deferred**: High (long-running sessions will eventually overflow provider windows, and compaction decisions will be hard to audit if hidden inside provider adapters)
**Status**: Proposed
**Depends on**: `roadmap/v0.30-agent/p5-session-run-model.md`, `roadmap/v0.30-agent/p6-turn-planner.md`, `roadmap/v0.30-agent/p7-run-traces-and-intervention.md`

## Goal

Add explicit, deterministic SDK support for context compaction.

Primary outcome:

1. compaction is an auditable workflow decision, not hidden adapter behavior,
2. full session history remains durable and replayable,
3. the active model window can be replaced by compacted summaries or provider-native compaction artifacts,
4. the workflow can react to provider context pressure signals such as context-limit failures, provider compaction recommendations, provider-returned compaction artifacts, and usage returned by successful generations,
5. OpenAI, Anthropic, and generic model backends can share the same SDK-level control flow while using different provider capabilities underneath,
6. token counting exists as an optional facility, but it is not required on the normal generation path,
7. session and run state can show that compaction is pending or in progress, instead of looking idle or failed while context maintenance runs.

## Problem

The current session model keeps transcript refs and turn-planner inputs close together. That works for short sessions, but it does not scale:

1. transcript history grows without bound,
2. `TurnBudget` exists but currently has only rough estimates,
3. provider response usage tells us what just happened and can be used as a cheap trigger for future compaction,
4. failed generation receipts can reveal that the active window exceeded a provider limit and should be compacted before retry,
5. local tokenizer estimates and provider token-count APIs are useful, but they are not worth making the hot path depend on extra I/O for every candidate turn,
6. provider-specific compaction APIs can produce artifacts that are not portable across providers,
7. provider-native compaction artifacts may be opaque, encrypted, or typed content blocks rather than plain chat messages,
8. current LLM request/response normalization can drop provider-native context-management items if it only preserves assistant text, tool calls, and reasoning,
9. if compaction mutates session state implicitly inside a normal `sys/llm.generate@1`, session state can silently diverge from what the planner and trace claim happened.

The SDK needs a durable split between:

1. full audit history, and
2. the active context window sent to the next model call.

## Provider Fit

Provider context-management APIs are temporally sensitive. Re-check current provider docs before implementing adapter-specific behavior, but the SDK contract should assume that provider-native compaction exists and can change shape over time.

### OpenAI

OpenAI Responses supports provider-native token counting and compaction.

Relevant capabilities:

1. exact input token counting for Responses-style payloads,
2. server-side compaction from a Responses create request through `context_management` and a compact threshold,
3. standalone compaction through the Responses compact endpoint,
4. opaque encrypted compaction items that can carry prior context forward using fewer tokens,
5. compacted windows that may include both a compaction item and retained input/output items.

SDK implication:

1. OpenAI token counting is a good optional backend for `sys/llm.count_tokens@1`.
2. OpenAI standalone compaction is a good backend for `sys/llm.compact@1`.
3. OpenAI server-side compaction may be enabled through `sys/llm.generate@1` provider options, but any returned compaction item must be surfaced in the receipt and trace.
4. OpenAI compaction artifacts are provider-native active-window items, not generic AOS summaries.
5. Standalone compact output should be treated as the canonical next OpenAI active window. Do not re-prune or reinterpret it as a single summary ref.
6. OpenAI usage and context-pressure errors should be enough to drive the normal compaction loop without a count request before every generation.

### Anthropic

Anthropic has token counting, server-side compaction, and context editing. These are related but should map to different SDK concepts.

Relevant capabilities:

1. provider token counting for Messages-style requests,
2. server-side compaction through `context_management.edits` using a compaction strategy, trigger threshold, optional pause-after-compaction, and returned compaction blocks,
3. context-editing options that can clear older tool results, tool uses, and thinking blocks during request processing,
4. context-management response metadata that reports applied edits and token deltas,
5. usage iteration metadata when a request performs both compaction sampling and normal message sampling,
6. client-side history can remain the durable source of truth even when provider-side context editing ignores or clears portions of the rendered prompt.

SDK implication:

1. Anthropic token counting is a good optional backend for `sys/llm.count_tokens@1`; mark its quality as provider estimate unless the provider guarantees exactness for the rendered request.
2. Anthropic provider-native compaction is a valid backend for `sys/llm.compact@1` when explicitly requested.
3. Anthropic server-side compaction may also be enabled through `sys/llm.generate@1` provider options, but returned compaction blocks must be surfaced in receipts/traces and represented as provider-native active-window items if the workflow applies them.
4. Anthropic context editing should be modeled as provider request shaping on `sys/llm.generate@1` or `sys/llm.count_tokens@1`, with applied edits recorded in receipt metadata. Context editing alone is not a durable AOS state mutation.
5. AOS-owned summaries are still useful for portability, unsupported models, custom summary prompts, and workflows that need inspectable summaries rather than provider-native blocks.

### Generic Providers

Most providers will not have durable compaction APIs.

SDK implication:

1. `sys/llm.count_tokens@1` can use provider token-count APIs when available, a provider tokenizer when available, or a marked estimate otherwise.
2. `sys/llm.compact@1` should support AOS-owned summarization through an LLM call or a deterministic summarizer when available.
3. compaction receipts must say whether the result is provider-native, AOS-summary, mixed, local-estimate-derived, or unavailable.
4. generic providers should never receive provider-native artifacts unless compatibility metadata says they can render them.

## Design Stance

### 1) Compaction is the main feature

P11 should not make every model call pay an extra provider-token-count round trip.

The normal planner/finalizer loop should use cheap local signals:

1. active-window item counts and message/ref counts,
2. known rough token estimates already attached to context inputs,
3. usage returned by previous `sys/llm.generate@1` receipts,
4. provider context-pressure failures from attempted generation,
5. provider compaction recommendations or returned compaction artifacts when available,
6. manual/operator requested compaction.

`sys/llm.count_tokens@1` is still useful, but it should be an explicit diagnostic or planning effect used when the workflow asks for more confidence. It is not the default prerequisite for generation.

### 2) Add explicit LLM effects

Add two effects:

1. `sys/llm.count_tokens@1`
2. `sys/llm.compact@1`

Do not overload ordinary assistant generation as the only context-management boundary.

`sys/llm.generate@1` can still expose provider-native context-management options, and AOS-owned summary compaction may be implemented by making an ordinary LLM generation call with a summarization prompt. The important boundary is that the workflow records this as a compaction operation, surfaces the result in receipts/traces, and applies the active-window update explicitly.

`sys/llm.generate@1` also needs enough receipt metadata to preserve provider-native context-management results returned during generation. If a provider returns compaction items, compaction blocks, applied context edits, or usage iterations, the adapter must not flatten them away.

### 3) Token counting is optional and non-mutating

`sys/llm.count_tokens@1` is non-mutating.

It accepts a provider/model plus refs or typed active-window items for the same rendered inputs the planner wants to send to `sys/llm.generate@1`.

It should be available for diagnostics, explicit budget checks, offline tuning, and high-risk turns, but it should not be required before every `sys/llm.generate@1`. Exact counts are useful; paying provider I/O latency for them on the common path is not.

The receipt should return:

1. total input tokens,
2. optional original input tokens before provider-side context management,
3. optional per-lane or per-ref counts when the provider/backend can support them,
4. tool/schema token contribution when available,
5. provider/model id used for counting,
6. count quality: `Exact`, `ProviderEstimate`, `LocalEstimate`, or `Unknown`,
7. provider metadata for diagnostics.

The planner may use this to decide whether to proceed, drop lower-priority inputs, or compact. A missing or stale count should not by itself block generation if the active-window policy allows trying the request.

### 4) Compaction is a workflow state transition

`sys/llm.compact@1` is an explicit effect because compaction changes the active model window.

This effect is the workflow-visible compaction boundary, not necessarily a provider-native API. Implementations can use:

1. provider-native compaction APIs when available,
2. a normal `sys/llm.generate@1` summarization call plus AOS-owned summary metadata,
3. a deterministic/local summarizer when available,
4. a scripted harness response for deterministic tests.

It accepts:

1. provider/model and compaction strategy,
2. source window items or a range in the full transcript ledger,
3. preserve refs/items that must remain verbatim,
4. recent-tail policy,
5. target token budget,
6. optional provider-specific parameters.

The receipt should return:

1. compaction artifact refs,
2. artifact kind: `AosSummary`, `ProviderNative`, or `Mixed`,
3. compacted source range,
4. compacted-through marker,
5. recommended replacement active-window items,
6. input/output token usage when available,
7. provider metadata,
8. warnings when the result is non-portable or approximate.

The workflow, not the adapter, applies the receipt to session state.

### 5) Active window items are typed

Do not model the active window as only `Vec<HashRef>`.

AOS needs an ordered active window whose items can represent:

1. normal message refs,
2. AOS summary refs,
3. provider-native compaction artifact refs,
4. provider raw input-item/window refs,
5. retained recent-tail message refs,
6. provider-specific reasoning or context-management artifacts when a backend requires them.

Each provider-native item needs compatibility metadata at creation time:

1. provider id,
2. provider API kind,
3. model or model-family compatibility,
4. provider artifact type,
5. creation operation id,
6. source range or source item refs when known,
7. whether the item is opaque, encrypted, or human-readable.

The planner and renderer must reject incompatible provider-native items rather than silently passing them to a different backend.

### 6) Keep full history separate from the active model window

Add an explicit state split:

1. transcript ledger: append-only, durable, auditable full session history,
2. active window: typed items eligible for the next model request,
3. compaction records: append-only records of compacted ranges and artifacts,
4. compacted-through marker: source history position represented by compaction artifacts,
5. recent tail: un-compacted recent message refs kept verbatim.

Old transcript blobs are never deleted as part of compaction.

### 7) Compaction has pending operation state

Session state should explicitly say when context maintenance is pending or in progress.

Generic pending effects are not sufficient for operators or deterministic control reads. Add a small pending context operation record that can represent:

1. `Idle`,
2. `NeedsCompaction`,
3. `CountingTokens`,
4. `Compacting`,
5. `ApplyingCompaction`,
6. `Failed`.

The record should include operation id, candidate plan id when applicable, reason, strategy, source range/items, emitted effect intent or params hash, started/updated times, and typed failure details. A run may separately point at the operation it is blocked on. Do not turn session status into `Compacting`; the session can remain open while a run is blocked on context maintenance.

### 8) Planning and finalization share compaction triggers

Pre-turn planning and post-turn finalization have different responsibilities.

Pre-turn planner responsibilities:

1. build candidate turn plan from pinned, durable, runtime, skill, memory, active-window, recent-tail, and user inputs,
2. use cheap local policy to drop optional low-priority inputs when the active window is clearly too large,
3. return `CompactContext` when existing session state already says compaction is required before generation,
4. return `CountTokens` only for explicit diagnostics, high-risk preflight, missing model-window metadata, or planner strategies that choose to pay the latency,
5. dispatch `sys/llm.generate@1` when the request is plausible.

Post-turn finalizer/workflow responsibilities:

1. if generation fails with a context-limit or provider-required compaction signal, record `ContextPressureObserved`, set a compaction prerequisite, and retry after compaction,
2. if generation succeeds but returned usage crosses a configured high-water mark, schedule compaction before the next turn,
3. if provider receipt metadata includes a compaction recommendation or provider-native compaction artifact, surface it in trace and decide whether to apply it,
4. record token usage and provider context-management metadata for later planning.

This keeps compaction observable through `TurnPlanned`, post-turn trace entries, and pending operation state without making the turn planner perform hidden work.

### 9) Provider-native compaction is not portable memory

Provider-native compaction artifacts are valid active-window inputs only for the compatible provider/model family.

They should not be treated as:

1. human-readable summaries,
2. cross-provider memory,
3. source-of-truth history,
4. durable replacement for transcript refs.

When provider-native artifacts exist, the planner must know their provider compatibility before selecting them for a turn.

### 10) AOS summaries are portable but lossy

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

### `ActiveWindowItem`

Fields:

1. `item_id`: deterministic stable id,
2. `kind`: `MessageRef | AosSummaryRef | ProviderNativeArtifactRef | ProviderRawWindowRef | Custom`,
3. `ref`: hash ref for the item payload,
4. `lane`: optional turn input lane metadata,
5. `source_range`: optional transcript ledger range represented by this item,
6. `source_refs`: optional explicit source refs,
7. `provider_compatibility`: optional provider compatibility metadata,
8. `estimated_tokens`: optional deterministic estimate,
9. `metadata`: freeform bounded metadata.

### `ProviderCompatibility`

Fields:

1. `provider`: provider id,
2. `api_kind`: provider API kind or rendering mode,
3. `model`: optional exact model id,
4. `model_family`: optional model-family id,
5. `artifact_type`: provider artifact type,
6. `opaque`: boolean,
7. `encrypted`: boolean.

### `LlmCountTokensParams`

Fields:

1. `provider`: provider id,
2. `model`: model id,
3. `window_items`: active-window items or a ref to a rendered candidate window,
4. `message_refs`: compatibility field for simple message-ref windows,
5. `tool_definitions_ref`: optional tool definitions blob ref,
6. `response_format_ref`: optional response format ref,
7. `provider_options_ref`: optional provider options ref,
8. `rendering_profile`: optional provider/API rendering mode,
9. `candidate_plan_id`: optional plan identity for stale-result protection,
10. `metadata`: freeform request metadata.

### `LlmCountTokensReceipt`

Fields:

1. `input_tokens`: optional integer,
2. `original_input_tokens`: optional integer before provider-side context management,
3. `counts_by_ref`: optional list of `{ ref, tokens, quality }`,
4. `tool_tokens`: optional integer,
5. `response_format_tokens`: optional integer,
6. `quality`: `Exact | ProviderEstimate | LocalEstimate | Unknown`,
7. `provider`: provider id,
8. `model`: model id,
9. `candidate_plan_id`: optional plan identity echoed from params,
10. `provider_metadata_ref`: optional CBOR/JSON ref,
11. `warnings`: list of strings or typed warning codes.

### `LlmCompactParams`

Fields:

1. `provider`: provider id,
2. `model`: model id,
3. `strategy`: `ProviderNative | AosSummary | Auto`,
4. `source_range`: compactable transcript ledger range,
5. `source_items`: optional explicit active-window items,
6. `source_refs`: compatibility field for explicit refs,
7. `preserve_items`: active-window items that must remain verbatim,
8. `preserve_refs`: compatibility field for refs that must remain verbatim,
9. `recent_tail_count`: minimum recent messages to keep verbatim,
10. `target_input_tokens`: optional target budget,
11. `summary_prompt_ref`: optional prompt ref for AOS summary strategy,
12. `provider_options_ref`: optional provider-specific options,
13. `candidate_plan_id`: optional plan identity for stale-result protection,
14. `operation_id`: workflow-assigned context operation id,
15. `metadata`: freeform request metadata.

### `LlmCompactReceipt`

Fields:

1. `operation_id`: workflow-assigned context operation id,
2. `artifact_refs`: refs to summary/provider-native artifacts,
3. `artifact_kind`: `AosSummary | ProviderNative | Mixed`,
4. `compacted_range`: transcript ledger range represented by the artifacts,
5. `compacted_through`: transcript ledger sequence marker,
6. `active_window_items`: recommended replacement items for the compacted portion,
7. `active_window_refs`: compatibility replacement refs for simple windows,
8. `usage`: optional usage record, including provider compaction iterations when available,
9. `provider`: provider id,
10. `model`: model id,
11. `provider_metadata_ref`: optional CBOR/JSON ref,
12. `warnings`: list of strings or typed warning codes.

### `ContextPressureRecord`

Fields:

1. `reason`: `ProviderContextLimit | ProviderRecommended | UsageHighWater | LocalWindowPolicy | Manual | CountTokensOverBudget`,
2. `provider`: optional provider id,
3. `model`: optional model id,
4. `candidate_plan_id`: optional candidate plan id,
5. `observed_usage`: optional usage record,
6. `error_kind`: optional provider error kind,
7. `error_ref`: optional provider error metadata ref,
8. `recommended_strategy`: optional compaction strategy,
9. `observed_at_ns`: deterministic observed time.

### `ContextOperationState`

Fields:

1. `operation_id`: deterministic id,
2. `phase`: `NeedsCompaction | CountingTokens | Compacting | ApplyingCompaction | Failed`,
3. `reason`: context pressure reason,
4. `candidate_plan_id`: optional candidate plan id,
5. `strategy`: chosen compaction strategy,
6. `source_range`: optional source range,
7. `source_items_ref`: optional source item list ref,
8. `effect_intent_id`: optional emitted effect intent id,
9. `params_hash`: optional emitted params hash,
10. `failure`: optional typed failure,
11. `started_at_ns`: deterministic start time,
12. `updated_at_ns`: deterministic update time.

### `CompactionRecord`

Fields:

1. `operation_id`: context operation id,
2. `strategy`: chosen compaction strategy,
3. `artifact_kind`: `AosSummary | ProviderNative | Mixed`,
4. `artifact_refs`: artifact refs,
5. `source_range`: compacted source range,
6. `source_refs`: optional source refs,
7. `active_window_items`: replacement active-window items,
8. `provider_compatibility`: optional provider compatibility metadata,
9. `usage`: optional usage record,
10. `created_at_ns`: deterministic creation time,
11. `warnings`: bounded warning codes.

## Proposed State

Add or evolve session state toward a session-scoped `context_state`:

1. `transcript_ledger_ref`: durable/chunked append-only transcript ledger,
2. `active_window_items`: ordered typed items selected as the base window for the next turn,
3. `compaction_records`: append-only compaction history,
4. `compacted_through`: optional transcript ledger sequence marker,
5. `pending_context_operation`: optional context operation state,
6. `last_llm_usage`: latest generation usage for cheap compaction heuristics,
7. `last_context_pressure`: optional latest provider context-limit failure or recommendation,
8. `last_token_count`: optional latest count result for diagnostics,
9. `last_compaction`: optional latest compaction record for diagnostics.

Run state should be able to point at a blocking context operation:

1. `blocked_on_context_operation`: optional operation id,
2. `pending_llm_turn_refs` remains untouched until compaction is applied or explicitly cancelled,
3. run lifecycle remains `Running` or `WaitingInput` according to existing semantics; do not add a terminal compaction lifecycle.

Existing `transcript_message_refs` can be migrated into the ledger/window split. Because this SDK is still experimental, prefer the clean split over compatibility shims.

## Proposed Planner Prerequisites

Extend turn planning prerequisites with:

1. `CompactContext`
2. `CountTokens`

`CountTokens` should be optional and explicit. It should include the candidate plan identity so stale token-count receipts cannot be applied to a different plan.

`CompactContext` should include:

1. source range,
2. chosen strategy,
3. budget target,
4. candidate plan identity,
5. reason: `ProviderContextLimit`, `ProviderRecommended`, `UsageHighWater`, `LocalWindowPolicy`, `Manual`, or `CountTokensOverBudget`,
6. operation id when an operation is already pending.

Prerequisites request work. They do not apply state changes by themselves.

## Trace Events

Add trace coverage for:

1. `CompactionRequested`,
2. `CompactionReceived`,
3. `ActiveWindowUpdated`,
4. `ContextPressureObserved`,
5. `TokenCountRequested`,
6. `TokenCountReceived`,
7. `ContextOperationStateChanged`.

`TurnPlanned` should continue to show selected input refs/items and token budget status.

Compaction traces should include refs and metadata, not inline prompt/history text.

## First Cut

Implement the narrowest useful version:

1. add contracts for `sys/llm.compact@1`, `ActiveWindowItem`, `ProviderCompatibility`, `ContextPressureRecord`, `ContextOperationState`, and `CompactionRecord`,
2. add state fields for `active_window_items`, `compaction_records`, `pending_context_operation`, latest generation usage, latest context-pressure diagnostics, and latest compaction diagnostics,
3. add `CompactContext` planner prerequisites and run blocking state,
4. make the workflow able to observe context-limit generation failures and turn them into context pressure plus a compaction prerequisite instead of terminal run failure,
5. make the workflow able to trigger compaction before the next turn from returned generation usage crossing a high-water mark,
6. make the workflow able to apply a scripted compaction receipt,
7. add deterministic harness tests that script context-limit failures, usage-triggered compaction, pending compaction state, and compaction receipts,
8. add contracts for `sys/llm.count_tokens@1` for schema completeness, but adapter implementation can be minimal or stubbed in the first cut,
9. allow the first implementation to back `sys/llm.compact@1` with a scripted or ordinary LLM summarization path,
10. preserve provider-native artifacts from `sys/llm.generate@1` receipts as raw refs/metadata even if full provider adapter compaction support is deferred,
11. keep live provider optimization minimal until the planner/state path is proven.

Do not implement a full summarizer, memory engine, full provider compatibility matrix, or UI in the first cut.

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
10. sophisticated rolling-window policies beyond summary plus recent tail,
11. mandatory provider-token-count preflight before generation,
12. complete provider-native compaction adapters for every backend.

## Acceptance Criteria

1. [ ] compaction is represented as an explicit workflow operation with receipts/traces and active-window updates, even when the compaction artifact is produced by an ordinary `sys/llm.generate@1` summarization call,
2. [ ] turn planning can request compaction before generation,
3. [ ] post-turn finalization can schedule compaction from usage high-water marks or provider-returned context-management metadata,
4. [ ] a context-limit `sys/llm.generate@1` failure can produce a traceable compaction prerequisite instead of silently failing the session,
5. [ ] session/run state can indicate that context compaction is pending or in progress,
6. [ ] full transcript history remains durable after compaction,
7. [ ] active model window can be replaced by typed compaction artifacts plus recent tail,
8. [ ] compaction records are append-only and traceable,
9. [ ] provider-native compaction artifacts are marked provider-specific and rejected for incompatible providers/models,
10. [ ] provider-native compaction artifacts returned from ordinary generation are surfaced in receipts/traces instead of flattened away,
11. [ ] AOS summaries are represented as normal refs with source ranges and usage metadata,
12. [ ] token counting is represented as optional `sys/llm.count_tokens@1` and is not required on the default generation path,
13. [ ] deterministic tests can script context-limit failures, usage-triggered compaction, pending operation state, and compaction receipts without live provider credentials.
