# AOS Smoke Fixtures

These fixtures are the numbered smoke demos executed by `aos-smoke`.

Grouping:
- `00`-`19`: core/kernel/system fixtures (including trace observability conformance).
- `20+`: Agent SDK conformance fixtures.

| No. | Slug           | Summary                      |
| --- | -------------- | ---------------------------- |
| 00  | counter        | Deterministic reducer SM     |
| 01  | hello-timer    | Reducer micro-effect demo    |
| 02  | blob-echo      | Reducer blob round-trip      |
| 03  | fetch-notify   | Plan-triggered HTTP demo     |
| 04  | aggregator     | Fan-out plan join demo       |
| 05  | chain-comp     | Multi-plan saga + refund     |
| 06  | safe-upgrade   | Governance shadow/apply demo |
| 07  | llm-summarizer | HTTP + LLM summarization     |
| 08  | retry-backoff  | Reducer retry with timer     |
| 09  | workspaces     | Workspace plans + caps demo  |
| 10  | trace-failure-classification | Trace-get/diagnose failure conformance |
| 20  | agent-session  | SDK session lifecycle replay |

Run with:
- `cargo run -p aos-smoke --`
- `cargo run -p aos-smoke -- <slug>`
- `cargo run -p aos-smoke -- all` (core fixtures `00`-`19`)
- `cargo run -p aos-smoke -- all-agent` (Agent SDK fixtures `20+`)
