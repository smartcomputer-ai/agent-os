# aos-agent-eval

Prompt-level eval harness for `aos.agent/SessionWorkflow@1` with live tool execution.

## What it does

- Creates one clean temp world per CLI invocation, then reuses it for all cases/runs.
- Allocates a fresh session + workspace directory per attempt for isolation.
- Seeds case files into the per-attempt workspace.
- Boots the SDK session workflow using fixture AIR + imported SDK AIR defs.
- Dispatches real host tools (`host.fs.*`, `host.exec`, etc.) through host adapters.
- Executes `llm.generate` live through provider APIs, then feeds receipts back to kernel.
- Asserts case expectations (tool usage, tool output content, filesystem outcomes).
- Supports pass-rate thresholds for flaky/probabilistic tasks.

## Commands

- List cases:
  - `cargo run -p aos-agent-eval -- list`
- Run one case:
  - `cargo run -p aos-agent-eval -- case read-write-token`
- Run all cases:
  - `cargo run -p aos-agent-eval -- all`

## Global options

- `--provider openai|anthropic` (default: `openai`)
- `--model <name>`
- `--runs <n>` override per-case runs
- `--entry direct|demiurge` (default: `direct`)

Examples:

- `cargo run -p aos-agent-eval -- all --provider openai --model gpt-5.3-codex --runs 3`
- `cargo run -p aos-agent-eval -- case edit-file --provider openai`
- `cargo run -p aos-agent-eval -- case edit-file --entry demiurge`

## Case files

Case files live in `crates/aos-agent-eval/cases/*.json`.

Top-level fields:

- `id`: unique case id
- `description`: short human-readable description
- `prompt`: user prompt sent to the agent
- `setup.files[]`: files to seed into the per-run workspace
- `expect.tool_called[]`: canonical tool ids expected to be used (for example `host.fs.read_file`)
- `expect.assistant_contains[]`: substrings expected in assistant output
- `expect.tool_output_contains[]`: substrings expected in mapped tool-output JSON
- `expect.files[]`: file assertions (`exists`, `contains`, `equals`)
- `eval.runs`: number of attempts
- `eval.min_pass_rate`: pass threshold from `0.0` to `1.0`
- `eval.max_steps`: max effect-dispatch rounds before safety trip
- `run.*`: run-time tool/profile overrides mapped to `SessionConfig`

Notes:

- Tool profiles and overrides use canonical `tool_id` values (`host.*`).
- LLM-facing tool names (`read_file`, `shell`, `edit_file`, etc.) come from the registry and are resolved back to `tool_id` for assertions.

## Provider credentials

Environment variables are read from process env first, then `.env` files.

- OpenAI: `OPENAI_API_KEY`, optional `OPENAI_BASE_URL`
- Anthropic: `ANTHROPIC_API_KEY`, optional `ANTHROPIC_LIVE_MODEL`, optional `ANTHROPIC_BASE_URL`

Default OpenAI model is `gpt-5.3-codex` (override with `--model`).
