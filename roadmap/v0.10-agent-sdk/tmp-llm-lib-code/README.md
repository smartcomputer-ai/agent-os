# forge-llm

Unified LLM client library for Forge. This crate implements the multi-provider spec in `spec/01-unified-llm-spec.md`.

## Current provider coverage

- OpenAI: native Responses API + OpenAI-compatible Chat Completions adapter.
- Anthropic: native Messages API adapter.
- Gemini: currently deferred in this repository roadmap.

## API surface

Main modules:

- `client`: provider registration/routing and middleware-capable core client.
- `high_level`: convenience APIs (`generate`, `stream`, `generate_object`, `stream_object`).
- `types`: unified request/response/message/tool data model.

High-level usage starts from `GenerateOptions` and the helpers in `high_level`.

## Configuration model

- `Client::from_env()` auto-registers configured providers from environment variables.
- You can also register adapters programmatically (recommended in tests and custom setups).
- Requests are provider-agnostic by default; set `request.provider` (or options provider) to pin a provider explicitly.

Environment keys currently used:

- OpenAI: `OPENAI_API_KEY` (`OPENAI_BASE_URL`, `OPENAI_ORG_ID`, `OPENAI_PROJECT_ID` optional)
- Anthropic: `ANTHROPIC_API_KEY` (`ANTHROPIC_BASE_URL` optional)

## Provider options and escape hatches

Use `Request.provider_options` (or `GenerateOptions.provider_options`) for provider-specific features.

Example keys already supported:

- OpenAI: `provider_options.openai.*` merged into Responses body.
- Anthropic:
  - `provider_options.anthropic.beta_headers` / `beta_features` -> `anthropic-beta` header.
  - `provider_options.anthropic.auto_cache = false` disables automatic prompt-cache breakpoint injection.
  - other `provider_options.anthropic.*` keys are passed into the Messages API request body.

## Behavior notes

- Anthropic strict alternation is enforced by adapter-side message merging.
- Anthropic tool results are translated into user-role `tool_result` content blocks.
- Anthropic defaults `max_tokens` to `4096` if unset.
- Prompt caching for Anthropic is auto-injected unless opted out.
- Streaming returns normalized unified events (`StreamStart`, `TextDelta`, `ToolCall*`, `Finish`, etc.).
- Streaming errors are emitted as `StreamEventType::Error` events before the stream closes.

## Low-level retries

When building your own loop on top of `Client.complete()` / `Client.stream()`, use the SDK retry helper:

```rust
use forge_llm::errors::{RetryPolicy, retry_async};

let policy = RetryPolicy::default();
let response = retry_async(&policy, || {
    let client = client.clone();
    let request = request.clone();
    async move { client.complete(request).await }
})
.await?;
```

This keeps retry behavior consistent with SDK defaults (retryable errors + backoff + retry-after handling).

## Executable references

The most reliable usage references are the test files:

- Mocked OpenAI integration: `crates/forge-llm/tests/openai_integration_mocked.rs`
- Mocked Anthropic integration: `crates/forge-llm/tests/anthropic_integration_mocked.rs`
- Cross-provider conformance tests: `crates/forge-llm/tests/cross_provider_conformance.rs`
- Optional live OpenAI tests: `crates/forge-llm/tests/openai_live.rs`
- Optional live Anthropic tests: `crates/forge-llm/tests/anthropic_live.rs`

## Build

```
cargo build -p forge-llm
```

## Tests

Run the crate test suite:

```
cargo test -p forge-llm
```

Run live OpenAI integration tests (ignored by default):

```
RUN_LIVE_OPENAI_TESTS=1 cargo test -p forge-llm --test openai_live -- --ignored
```

Live tests require `OPENAI_API_KEY` (read from environment or from project-root `.env`).
Optional live-test settings:

- `OPENAI_LIVE_MODEL` (default: `gpt-5-mini`)
- `OPENAI_BASE_URL`
- `OPENAI_ORG_ID`
- `OPENAI_PROJECT_ID`

Run live Anthropic integration tests (ignored by default):

```
RUN_LIVE_ANTHROPIC_TESTS=1 cargo test -p forge-llm --test anthropic_live -- --ignored
```

Live Anthropic tests require `ANTHROPIC_API_KEY` (read from environment or from project-root `.env`).
Optional live-test settings:

- `ANTHROPIC_LIVE_MODEL` (default: `claude-sonnet-4-5`)
- `ANTHROPIC_BASE_URL`
