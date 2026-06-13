# Debugging The Chat TUI As An Agent

Use this note when an AI coding agent needs to run, inspect, or regress-test the Ratatui chat TUI.

## Preferred Tools

1. **Ratatui `TestBackend` snapshots**

   Use this for deterministic UI tests. It is the right tool for layout, cells, composer behavior, pickers, overlays, and reducer behavior.

   ```sh
   cargo test -p aos-cli tui
   ```

2. **`tmux` live smoke harness**

   Use this for real terminal behavior and end-to-end chat flow: raw mode, bracketed paste, Enter handling, journal follow, and visible transcript updates.

   ```sh
   tmux kill-session -t aos-chat-smoke 2>/dev/null || true
   tmux new-session -d -s aos-chat-smoke -x 100 -y 30 \
     'cd /Users/lukas/dev/aos && target/debug/aos chat --new'

   sleep 2
   tmux capture-pane -t aos-chat-smoke -p -S -30

   tmux send-keys -t aos-chat-smoke 'Say pong in one short sentence.' Enter
   sleep 12
   tmux capture-pane -t aos-chat-smoke -p -S -30

   tmux kill-session -t aos-chat-smoke
   ```

   `capture-pane -p` gives the agent a readable visible-frame snapshot. Use `-S -200` when scrollback matters.

3. **PTY fallback**

   If `tmux` is unavailable, run the TUI in a PTY through the agent shell and send keystrokes. This is less reliable because inline viewport setup can depend on terminal cursor-position reports, and ANSI output is harder to read than `tmux capture-pane`.

## Runtime Setup Checklist

Before blaming the TUI, verify the runtime:

```sh
target/debug/aos node status --pretty
target/debug/aos profile show --pretty
target/debug/aos chat sessions --pretty
```

For Demiurge/OpenAI smoke tests, the node must be able to resolve the LLM secret used by the effect adapter. If the effect receipt says `raw/api_key secret ref unresolved: llm/openai_api@1`, sync or add the compatible binding before retrying:

```sh
target/debug/aos universe secret sync --local-root worlds/demiurge --pretty

set -a; source worlds/demiurge/.env; set +a
target/debug/aos universe secret binding set 'llm/openai_api@1' node_secret_store --pretty
target/debug/aos universe secret version add 'llm/openai_api@1' --text "$OPENAI_API_KEY" --pretty
```

Do not print secret values in logs or final reports.

## Useful Inspection Commands

After a TUI smoke, find the session and inspect the projected transcript:

```sh
target/debug/aos chat sessions --pretty
target/debug/aos chat history --session <session_id> --pretty
```

Inspect journal records around a run:

```sh
target/debug/aos world journal head --pretty
target/debug/aos world journal tail --from <seq> --limit 30 --pretty
```

Inspect runtime summary:

```sh
target/debug/aos world status --pretty
target/debug/aos world trace summary --pretty
```

## What Good Looks Like

A successful `tmux` smoke should show the user message, assistant message, and status returning to the next-input state:

```text
AOS Chat  world d7ba6cde  session ecf9340f
user
  Say pong in one short sentence.

assistant
  Pong.

run 1 WaitingInput gpt-5.3-codex
...
waiting_input  gpt-5.3-codex  effort none
>
```

## Common Failure Modes

- **TUI starts but pane stays unchanged after input**: check `chat sessions` and `chat history`; the input may have submitted but the TUI did not refresh or follow correctly.
- **Visible `submit aos.agent session input` error but history later has output**: the submit path probably waited for flush or treated delayed acceptance as failure. The TUI should use immediate accept and follow journal progress.
- **Run remains `Running` forever**: inspect journal receipts and node status. The node may have crashed or the workflow may have trapped.
- **Effect receipt has unresolved API key**: fix node secret binding, especially `llm/openai_api@1` for current Demiurge/OpenAI smoke tests.
- **PTY init fails with cursor-position timeout**: prefer `tmux`; the TUI has a standard-terminal fallback, but `tmux capture-pane` is easier to reason about.

## Cleanup

Always kill smoke sessions after use:

```sh
tmux kill-session -t aos-chat-smoke 2>/dev/null || true
```

Do not leave long-running `exec_command` sessions open.
