import json
import os
import time

from aos_harness.agent import (
    BLOB_GET,
    agent_workflow,
    apply_llm_generate_ok,
    expect_llm_generate,
    find_effect,
    last_llm_usage,
    require_trace_kinds,
    respond_llm_output_blob,
    run_completed,
    run_history_summaries,
    run_requested,
    selected_message_refs,
    send_session_input,
    session_config,
    session_state,
)


SESSION_ID = "s-1"
INPUT_REF = "sha256:" + ("a" * 64)
OUTPUT_REF = "sha256:" + ("b" * 64)
VERBOSE = os.environ.get("AOS_HARNESS_VERBOSE", "").lower() not in {"", "0", "false", "no"}


class StepLogger:
    def __init__(self, enabled: bool):
        self.enabled = enabled
        self.started_at = time.perf_counter()

    def log(self, message: str) -> None:
        if not self.enabled:
            return
        elapsed = time.perf_counter() - self.started_at
        print(f"[+{elapsed:7.3f}s] {message}", flush=True)


def main():
    log = StepLogger(VERBOSE)
    log.log("opening reusable agent session workflow")
    harness = agent_workflow(build_profile="release")
    log.log("workflow harness ready")

    log.log("requesting a chat-only run")
    send_session_input(
        harness,
        SESSION_ID,
        1,
        run_requested(
            INPUT_REF,
            run_overrides=session_config(
                provider="openai",
                model="gpt-5.2",
                max_tokens=256,
            ),
        ),
    )
    harness.run_to_idle()

    llm = expect_llm_generate(
        harness.pull_effects(),
        provider="openai",
        model="gpt-5.2",
        message_refs=[INPUT_REF],
    )
    assert selected_message_refs(session_state(harness)) == [INPUT_REF]

    log.log("admitting deterministic llm.generate receipt")
    apply_llm_generate_ok(
        harness,
        llm,
        output_ref=OUTPUT_REF,
        provider_id="openai-responses",
        prompt_tokens=12,
        completion_tokens=4,
        total_tokens=16,
    )
    harness.run_to_idle()

    log.log("responding to llm output blob.get with fixed assistant text")
    output_blob_get = find_effect(harness.pull_effects(), BLOB_GET)
    assert output_blob_get["params"]["blob_ref"] == OUTPUT_REF
    respond_llm_output_blob(harness, output_blob_get, assistant_text="done")
    harness.run_to_idle()

    waiting_state = session_state(harness)
    require_trace_kinds(
        waiting_state,
        ["RunStarted", "TurnPlanned", "LlmRequested", "LlmReceived"],
    )
    assert last_llm_usage(waiting_state) == {
        "prompt_tokens": 12,
        "completion_tokens": 4,
        "total_tokens": 16,
        "reasoning_tokens": None,
        "cache_read_tokens": None,
        "cache_write_tokens": None,
    }

    log.log("completing run and checking replay/reopen state")
    send_session_input(harness, SESSION_ID, 5, run_completed())
    harness.run_to_idle()

    completed_state = session_state(harness)
    history = run_history_summaries(completed_state)
    assert len(history) == 1
    assert history[0]["outcome"]["output_ref"] == OUTPUT_REF
    assert history[0]["last_llm_usage"]["total_tokens"] == 16
    assert history[0]["trace_summary"]["last_kind"]["$tag"] == "RunFinished"

    reopened = harness.reopen()
    assert session_state(reopened) == completed_state

    print("agent_no_tool_llm.py: OK")
    print(
        json.dumps(
            {
                "run_history": history,
                "usage": history[0]["last_llm_usage"],
                "trace_summary": history[0]["trace_summary"],
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
