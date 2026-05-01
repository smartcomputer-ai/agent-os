from __future__ import annotations

import json

import pytest

from aos_harness import blob_put_ok
from aos_harness.agent import (
    BLOB_GET,
    LLM_GENERATE,
    agent_workflow,
    apply_llm_generate_ok,
    cause_ref,
    current_run,
    current_turn_plan,
    domain_event_run_cause,
    expect_llm_generate,
    find_effect,
    host_session_updated,
    last_llm_usage,
    llm_tool_call,
    require_trace_kinds,
    respond_blob_get_bytes,
    respond_llm_output_blob,
    respond_llm_tool_calls_blob,
    run_interrupt_requested,
    run_requested,
    run_start_requested,
    run_trace_kinds,
    selected_tool_ids,
    send_session_input,
    session_config,
    session_state,
    tool_executor_effect,
    tool_registry_set,
    tool_spec,
    trace_contains_kind,
)


SESSION_ID = "s-1"
INPUT_REF = "sha256:" + ("a" * 64)
PROMPT_REF = "sha256:" + ("b" * 64)
OUTPUT_REF = "sha256:" + ("c" * 64)
CALLS_REF = "sha256:" + ("d" * 64)
ARGS_REF = "sha256:" + ("e" * 64)
REASON_REF = "sha256:" + ("f" * 64)


def hash_ref(ch: str) -> str:
    return "sha256:" + (ch * 64)


@pytest.fixture
def harness():
    return agent_workflow(build_profile="release")


def host_registry() -> dict:
    return {
        "host.exec": tool_spec(
            tool_id="host.exec",
            tool_name="shell",
            tool_ref=hash_ref("1"),
            description="Execute a command in a ready host session.",
            args_schema_json=json.dumps(
                {
                    "type": "object",
                    "required": ["argv"],
                    "properties": {"argv": {"type": "array", "items": {"type": "string"}}},
                }
            ),
            mapper="HostExec",
            executor=tool_executor_effect("host.exec"),
            parallel_safe=False,
            resource_key="host.exec",
        ),
        "host.fs.apply_patch": tool_spec(
            tool_id="host.fs.apply_patch",
            tool_name="apply_patch",
            tool_ref=hash_ref("2"),
            description="Apply a patch in a ready host session.",
            args_schema_json=json.dumps(
                {
                    "type": "object",
                    "required": ["patch"],
                    "properties": {"patch": {"type": "string"}},
                }
            ),
            mapper="HostFsApplyPatch",
            executor=tool_executor_effect("host.fs.apply_patch"),
            parallel_safe=False,
            resource_key="host.fs.apply_patch",
        ),
    }


def install_host_registry(harness) -> None:
    send_session_input(
        harness,
        SESSION_ID,
        0,
        tool_registry_set(
            host_registry(),
            profiles={"host": ["host.exec", "host.fs.apply_patch"]},
            default_profile="host",
        ),
    )
    send_session_input(
        harness,
        SESSION_ID,
        1,
        host_session_updated(host_session_id="hs_1", host_session_status="Ready"),
    )
    harness.run_to_idle()


def run_config(**overrides) -> dict:
    config = session_config(
        provider="openai",
        model="gpt-5.2",
        max_tokens=256,
        **overrides,
    )
    return config


def intent_hash(effect: dict) -> bytes:
    value = effect["intent_hash"]
    if isinstance(value, bytes):
        return value
    return bytes(value)


def apply_ok(harness, effect: dict, payload: dict) -> None:
    harness.apply_receipt_object(harness.receipt_ok(intent_hash(effect), payload))


def settle_blob_puts(harness, effects: list[dict]) -> list[dict]:
    puts = [effect for effect in effects if effect.get("effect") == "sys/blob.put@1"]
    assert puts, "expected blob.put effects to settle"
    for idx, effect in enumerate(puts):
        receipt = blob_put_ok(
            harness,
            effect,
            blob_ref=hash_ref("0"),
            edge_ref=hash_ref(hex((idx % 15) + 1)[2:]),
            size=42,
        )
        harness.apply_receipt_object(receipt)
    harness.run_to_idle()
    return harness.pull_effects()


def start_chat_run(harness, *, input_ref: str = INPUT_REF, observed_at_ns: int = 1) -> list[dict]:
    send_session_input(
        harness,
        SESSION_ID,
        observed_at_ns,
        run_requested(input_ref, run_overrides=run_config()),
    )
    harness.run_to_idle()
    return harness.pull_effects()


def start_host_tool_run(harness) -> tuple[dict, list[dict]]:
    install_host_registry(harness)
    send_session_input(
        harness,
        SESSION_ID,
        2,
        run_requested(
            INPUT_REF,
            run_overrides=run_config(
                default_prompt_refs=[PROMPT_REF],
                default_tool_profile="host",
            ),
        ),
    )
    harness.run_to_idle()
    effects = harness.pull_effects()
    llm_effects = [effect for effect in effects if effect.get("effect") == LLM_GENERATE]
    if not llm_effects:
        effects = settle_blob_puts(harness, effects)
        llm_effects = [effect for effect in effects if effect.get("effect") == LLM_GENERATE]
    assert len(llm_effects) == 1
    return llm_effects[0], effects


def test_host_session_ready_tools_appear_in_turn_plan(harness):
    _llm, _effects = start_host_tool_run(harness)

    state = session_state(harness)
    plan = current_turn_plan(state)
    assert plan is not None
    assert selected_tool_ids(plan) == ["host.exec", "host.fs.apply_patch"]
    assert plan["message_refs"] == [PROMPT_REF, INPUT_REF]


def test_scripted_tool_call_path_queues_follow_up_llm_turn(harness):
    llm, _effects = start_host_tool_run(harness)

    apply_llm_generate_ok(
        harness,
        llm,
        output_ref=OUTPUT_REF,
        provider_id="openai-responses",
        finish_reason="tool_calls",
        prompt_tokens=20,
        completion_tokens=5,
        total_tokens=25,
    )
    harness.run_to_idle()

    output_blob_get = find_effect(harness.pull_effects(), BLOB_GET)
    respond_llm_output_blob(harness, output_blob_get, tool_calls_ref=CALLS_REF)
    harness.run_to_idle()

    calls_blob_get = find_effect(harness.pull_effects(), BLOB_GET)
    assert calls_blob_get["params"]["blob_ref"] == CALLS_REF
    respond_llm_tool_calls_blob(
        harness,
        calls_blob_get,
        [llm_tool_call(call_id="call-1", tool_name="shell", arguments_ref=ARGS_REF)],
    )
    harness.run_to_idle()

    args_blob_get = find_effect(harness.pull_effects(), BLOB_GET)
    assert args_blob_get["params"]["blob_ref"] == ARGS_REF
    respond_blob_get_bytes(harness, args_blob_get, b'{"argv":["pwd"]}')
    harness.run_to_idle()

    host_exec = find_effect(harness.pull_effects(), "sys/host.exec@1")
    assert host_exec["params"]["session_id"] == "hs_1"
    assert host_exec["params"]["argv"] == ["pwd"]
    apply_ok(harness, host_exec, {"status": "ok", "stdout": "/workspace\n", "exit_code": 0})
    harness.run_to_idle()

    follow_up_effects = harness.pull_effects()
    if not any(effect.get("effect") == LLM_GENERATE for effect in follow_up_effects):
        follow_up_effects = settle_blob_puts(harness, follow_up_effects)
    follow_up_llm = expect_llm_generate(follow_up_effects)
    assert INPUT_REF in follow_up_llm["params"]["message_refs"]

    state = session_state(harness)
    assert trace_contains_kind(state, "ToolCallsObserved")
    assert trace_contains_kind(state, "ToolBatchPlanned")
    assert trace_contains_kind(state, "EffectEmitted")
    assert last_llm_usage(state)["total_tokens"] == 25


def test_interrupt_finishes_run_without_follow_up_dispatch(harness):
    llm = expect_llm_generate(start_chat_run(harness))

    send_session_input(harness, SESSION_ID, 2, run_interrupt_requested(REASON_REF))
    harness.run_to_idle()

    apply_llm_generate_ok(
        harness,
        llm,
        output_ref=OUTPUT_REF,
        provider_id="openai-responses",
        prompt_tokens=10,
        completion_tokens=2,
        total_tokens=12,
    )
    harness.run_to_idle()

    output_blob_get = find_effect(harness.pull_effects(), BLOB_GET)
    respond_llm_output_blob(harness, output_blob_get, assistant_text="ignored after interrupt")
    harness.run_to_idle()

    state = session_state(harness)
    assert current_run(state) is None
    assert not harness.pull_effects()
    assert state["run_history"][0]["lifecycle"]["$tag"] == "Interrupted"
    assert state["run_history"][0]["outcome"]["interrupted_reason_ref"] == REASON_REF
    summary = state["run_history"][0]["trace_summary"]
    assert summary["last_kind"]["$tag"] == "RunFinished"


def test_domain_event_run_cause_records_provenance(harness):
    cause = domain_event_run_cause(
        kind="example/work_item_ready",
        schema="example/WorkItemReady@1",
        event_ref=hash_ref("9"),
        key="work-item-1",
        input_refs=[INPUT_REF],
        payload_schema="example/WorkItemReady@1",
        payload_ref=hash_ref("8"),
        subject_refs=[cause_ref("work_item", "work-item-1", hash_ref("7"))],
    )
    send_session_input(
        harness,
        SESSION_ID,
        1,
        run_start_requested(cause, run_overrides=run_config()),
    )
    harness.run_to_idle()

    llm = expect_llm_generate(harness.pull_effects())
    assert llm["params"]["message_refs"] == [hash_ref("8"), hash_ref("7"), INPUT_REF]
    run = current_run(session_state(harness))
    assert run is not None
    assert run["cause"]["kind"] == "example/work_item_ready"
    assert run["cause"]["origin"]["$tag"] == "DomainEvent"
    assert run["cause"]["origin"]["$value"]["schema"] == "example/WorkItemReady@1"
    assert run["cause"]["origin"]["$value"]["key"] == "work-item-1"
    assert run["cause"]["subject_refs"][0]["id"] == "work-item-1"
    require_trace_kinds(run, ["RunStarted", "TurnPlanned", "LlmRequested"])


def test_no_tool_completion_reopens_with_same_state(harness):
    llm = expect_llm_generate(start_chat_run(harness))
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

    output_blob_get = find_effect(harness.pull_effects(), BLOB_GET)
    respond_llm_output_blob(harness, output_blob_get, assistant_text="done")
    harness.run_to_idle()

    state = session_state(harness)
    require_trace_kinds(state, ["RunStarted", "TurnPlanned", "LlmRequested", "LlmReceived"])
    kinds = run_trace_kinds(state)
    assert kinds.index("RunStarted") < kinds.index("TurnPlanned")
    assert kinds.index("TurnPlanned") < kinds.index("LlmRequested")
    assert kinds.index("LlmRequested") < kinds.index("LlmReceived")
    assert last_llm_usage(state)["total_tokens"] == 16

    reopened = harness.reopen()
    assert session_state(reopened) == state
