# AOS Examples Ladder

These demos are a rung-by-rung ladder from a deterministic core to a self-upgrading world. Each example is a tiny, runnable integration test that forces the path the architecture cares about—deterministic replay, micro-effects, plan orchestration, governance, and finally LLM with caps/policy. The north star is a world an agent can safely modify via the constitutional loop (propose → shadow → approve → apply) while staying on the same deterministic journal.

What the ladder proves (in order):
- Deterministic reducer execution and replay on a journal.
- Reducers emitting one micro-effect (timer/blob) and handling receipts.
- Single-plan orchestration with HTTP and typed reducer boundaries.
- Deterministic fan-out/fan-in inside a plan.
- Multi-plan choreography and reducer-driven compensations.
- Governance loop with shadowed diffs and manifest swaps.
- Plans invoking LLM with capability constraints and policy gates, and secrets injection for LLM-based effects (e.g. API keys).

| No. | Slug          | Summary                         |
| --- | ------------- | -------------------------------- |
| 00  | counter       | Deterministic reducer SM         |
| 01  | hello-timer   | Reducer micro-effect demo        |
| 02  | blob-echo     | Reducer blob round-trip          |
| 03  | fetch-notify  | Plan-triggered HTTP demo         |
| 04  | aggregator    | Fan-out plan join demo           |
| 05  | chain-comp    | Multi-plan saga + refund         |
| 06  | safe-upgrade  | Governance shadow/apply demo     |
| 07  | llm-summarizer | HTTP + LLM summarization demo    |
| 08  | retry-backoff | Reducer-driven retry with timer  |
| 09  | worldfs-lab   | WorldFS view over notes + catalog |

Use the `aos-examples` CLI to list or run demos. Run them in order—the ladder is deliberate, and later steps assume the earlier capabilities and policies are already in place.
