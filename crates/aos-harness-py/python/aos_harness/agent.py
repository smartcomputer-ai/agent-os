from __future__ import annotations

import json
import tempfile
from pathlib import Path
from typing import TYPE_CHECKING, Any, Callable, Iterable, Mapping, Optional, Sequence

from ._core import canonical_cbor
from .fixtures import repo_root as resolve_repo_root
from .receipts import blob_get_ok, llm_generate_ok
from .types import BuildProfileName, EffectModeName, PathLike, ReceiptObject

if TYPE_CHECKING:
    from ._core import WorkflowHarness


SESSION_WORKFLOW = "aos.agent/SessionWorkflow@1"
SESSION_WORKFLOW_EVENT = "aos.agent/SessionWorkflowEvent@1"
SESSION_INPUT = "aos.agent/SessionInput@1"

LLM_GENERATE = "sys/llm.generate@1"
BLOB_GET = "sys/blob.get@1"


def _variant(tag: str, value: Optional[Mapping[str, Any]] = None) -> dict[str, Any]:
    if value is None:
        return {"$tag": tag}
    return {"$tag": tag, "$value": dict(value)}


def _unit_variant(tag: str) -> dict[str, Any]:
    return {"$tag": tag}


def _normalize_status(status: Optional[str | Mapping[str, Any]]) -> Optional[dict[str, Any]]:
    if status is None:
        return None
    if isinstance(status, str):
        return _unit_variant(status)
    return dict(status)


def _effect_params(effect: Mapping[str, Any]) -> Mapping[str, Any]:
    params = effect.get("params")
    if isinstance(params, Mapping):
        return params
    raise TypeError("effect.params must be an object")


def _tag(value: Any) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, Mapping):
        raw = value.get("$tag")
        if isinstance(raw, str):
            return raw
        if len(value) == 1:
            only = next(iter(value.keys()))
            if isinstance(only, str):
                return only
    raise TypeError(f"expected tagged value, got {value!r}")


def agent_workflow(
    source_root: Optional[PathLike] = None,
    *,
    workflow: str = SESSION_WORKFLOW,
    air_dir: Optional[PathLike] = None,
    workflow_dir: Optional[PathLike] = None,
    import_roots: Optional[list[str]] = None,
    force_build: bool = False,
    sync_secrets: bool = False,
    secret_bindings: Optional[dict[str, bytes | str]] = None,
    build_profile: BuildProfileName = "debug",
    effect_mode: EffectModeName = "scripted",
) -> "WorkflowHarness":
    """Open an agent session workflow with the generic workflow harness.

    By default this opens the reusable `crates/aos-agent` AIR package from the
    current repository through a one-bin shim for `SessionWorkflow`. Custom agents
    can pass their own `source_root`, `air_dir`, `workflow_dir`, and `workflow`
    while still reusing the input/effect helpers in this module.
    """

    from ._core import WorkflowHarness

    if source_root is None:
        root = resolve_repo_root() / "crates" / "aos-agent"
    else:
        root = Path(source_root).expanduser().resolve()

    resolved_air_dir = Path(air_dir).expanduser().resolve() if air_dir is not None else root / "air"

    if workflow_dir is not None:
        resolved_workflow_dir = Path(workflow_dir).expanduser().resolve()
        return WorkflowHarness.from_air_dir(
            workflow,
            str(resolved_air_dir),
            workflow_dir=str(resolved_workflow_dir),
            import_roots=import_roots,
            force_build=force_build,
            sync_secrets=sync_secrets,
            secret_bindings=secret_bindings,
            build_profile=build_profile,
            effect_mode=effect_mode,
        )

    if source_root is not None and (root / "workflow").is_dir():
        resolved_workflow_dir = root / "workflow"
        return WorkflowHarness.from_air_dir(
            workflow,
            str(resolved_air_dir),
            workflow_dir=str(resolved_workflow_dir),
            import_roots=import_roots,
            force_build=force_build,
            sync_secrets=sync_secrets,
            secret_bindings=secret_bindings,
            build_profile=build_profile,
            effect_mode=effect_mode,
        )

    if source_root is not None:
        resolved_workflow_dir = root
        return WorkflowHarness.from_air_dir(
            workflow,
            str(resolved_air_dir),
            workflow_dir=str(resolved_workflow_dir),
            import_roots=import_roots,
            force_build=force_build,
            sync_secrets=sync_secrets,
            secret_bindings=secret_bindings,
            build_profile=build_profile,
            effect_mode=effect_mode,
        )

    with tempfile.TemporaryDirectory(prefix="aos-agent-harness-") as temp:
        resolved_workflow_dir = _write_session_workflow_shim(Path(temp), root)
        return WorkflowHarness.from_air_dir(
            workflow,
            str(resolved_air_dir),
            workflow_dir=str(resolved_workflow_dir),
            import_roots=import_roots,
            force_build=force_build,
            sync_secrets=sync_secrets,
            secret_bindings=secret_bindings,
            build_profile=build_profile,
            effect_mode=effect_mode,
        )


def _write_session_workflow_shim(temp_root: Path, agent_root: Path) -> Path:
    src = temp_root / "src"
    src.mkdir(parents=True, exist_ok=True)
    repo_root = agent_root.parent.parent
    (temp_root / "Cargo.toml").write_text(
        "\n".join(
            [
                "[package]",
                'name = "aos-agent-harness-session-workflow"',
                'version = "0.1.0"',
                'edition = "2024"',
                "publish = false",
                "",
                "[dependencies]",
                f'aos-agent = {{ path = "{agent_root}" }}',
                (
                    f'aos-wasm-sdk = {{ path = "{repo_root / "crates" / "aos-wasm-sdk"}", '
                    'features = ["air-macros"] }'
                ),
                "",
            ]
        )
    )
    (src / "main.rs").write_text(
        "\n".join(
            [
                "#![allow(improper_ctypes_definitions)]",
                "#![no_std]",
                "",
                "extern crate alloc;",
                "",
                "use aos_agent::SessionWorkflow;",
                "use aos_wasm_sdk::aos_workflow;",
                "",
                "#[cfg(target_arch = \"wasm32\")]",
                "fn main() {}",
                "",
                "#[cfg(not(target_arch = \"wasm32\"))]",
                "fn main() {}",
                "",
                "aos_workflow!(SessionWorkflow);",
                "",
            ]
        )
    )
    return temp_root


def session_key(session_id: str) -> bytes:
    return canonical_cbor(session_id)


def session_input(
    session_id: str,
    observed_at_ns: int,
    input_kind: Mapping[str, Any],
) -> dict[str, Any]:
    return {
        "session_id": session_id,
        "observed_at_ns": observed_at_ns,
        "input": dict(input_kind),
    }


def send_session_input(
    harness: "WorkflowHarness",
    session_id: str,
    observed_at_ns: int,
    input_kind: Mapping[str, Any],
) -> dict[str, Any]:
    value = session_input(session_id, observed_at_ns, input_kind)
    harness.send_event(SESSION_INPUT, value)
    return value


def session_config(
    *,
    provider: str,
    model: str,
    reasoning_effort: Optional[Mapping[str, Any] | str] = None,
    max_tokens: Optional[int] = None,
    default_prompt_refs: Optional[Sequence[str]] = None,
    default_tool_profile: Optional[str] = None,
    default_tool_enable: Optional[Sequence[str]] = None,
    default_tool_disable: Optional[Sequence[str]] = None,
    default_tool_force: Optional[Sequence[str]] = None,
    default_host_session_open: Optional[Mapping[str, Any]] = None,
) -> dict[str, Any]:
    return {
        "provider": provider,
        "model": model,
        "reasoning_effort": _normalize_status(reasoning_effort),
        "max_tokens": max_tokens,
        "default_prompt_refs": list(default_prompt_refs)
        if default_prompt_refs is not None
        else None,
        "default_tool_profile": default_tool_profile,
        "default_tool_enable": list(default_tool_enable)
        if default_tool_enable is not None
        else None,
        "default_tool_disable": list(default_tool_disable)
        if default_tool_disable is not None
        else None,
        "default_tool_force": list(default_tool_force)
        if default_tool_force is not None
        else None,
        "default_host_session_open": dict(default_host_session_open)
        if default_host_session_open is not None
        else None,
    }


def run_requested(
    input_ref: str,
    *,
    run_overrides: Optional[Mapping[str, Any]] = None,
) -> dict[str, Any]:
    return _variant(
        "RunRequested",
        {
            "input_ref": input_ref,
            "run_overrides": dict(run_overrides) if run_overrides is not None else None,
        },
    )


def run_start_requested(
    cause: Mapping[str, Any],
    *,
    run_overrides: Optional[Mapping[str, Any]] = None,
) -> dict[str, Any]:
    return _variant(
        "RunStartRequested",
        {
            "cause": dict(cause),
            "run_overrides": dict(run_overrides) if run_overrides is not None else None,
        },
    )


def follow_up_input_appended(
    input_ref: str,
    *,
    run_overrides: Optional[Mapping[str, Any]] = None,
) -> dict[str, Any]:
    return _variant(
        "FollowUpInputAppended",
        {
            "input_ref": input_ref,
            "run_overrides": dict(run_overrides) if run_overrides is not None else None,
        },
    )


def run_steer_requested(instruction_ref: str) -> dict[str, Any]:
    return _variant("RunSteerRequested", {"instruction_ref": instruction_ref})


def run_interrupt_requested(reason_ref: Optional[str] = None) -> dict[str, Any]:
    return _variant("RunInterruptRequested", {"reason_ref": reason_ref})


def host_session_updated(
    *,
    host_session_id: Optional[str] = None,
    host_session_status: Optional[str | Mapping[str, Any]] = "Ready",
) -> dict[str, Any]:
    return _variant(
        "HostSessionUpdated",
        {
            "host_session_id": host_session_id,
            "host_session_status": _normalize_status(host_session_status),
        },
    )


def turn_observed(observation: Mapping[str, Any]) -> dict[str, Any]:
    return _variant("TurnObserved", dict(observation))


def tool_registry_set(
    registry: Mapping[str, Any],
    *,
    profiles: Optional[Mapping[str, Sequence[str]]] = None,
    default_profile: Optional[str] = None,
) -> dict[str, Any]:
    return _variant(
        "ToolRegistrySet",
        {
            "registry": dict(registry),
            "profiles": {key: list(value) for key, value in profiles.items()}
            if profiles is not None
            else None,
            "default_profile": default_profile,
        },
    )


def tool_profile_selected(profile_id: str) -> dict[str, Any]:
    return _variant("ToolProfileSelected", {"profile_id": profile_id})


def tool_mapper(name: str) -> dict[str, Any]:
    return _unit_variant(name)


def tool_executor_effect(effect: str) -> dict[str, Any]:
    return _variant("Effect", {"effect": effect})


def tool_executor_domain_event(schema: str) -> dict[str, Any]:
    return _variant("DomainEvent", {"schema": schema})


def tool_executor_host_loop(bridge: str = "host.tool") -> dict[str, Any]:
    return _variant("HostLoop", {"bridge": bridge})


def tool_spec(
    *,
    tool_id: str,
    tool_name: str,
    tool_ref: str,
    description: str = "",
    args_schema_json: str = "{}",
    mapper: str | Mapping[str, Any] = "HostExec",
    executor: Optional[Mapping[str, Any]] = None,
    parallel_safe: bool = False,
    resource_key: Optional[str] = None,
) -> dict[str, Any]:
    return {
        "tool_id": tool_id,
        "tool_name": tool_name,
        "tool_ref": tool_ref,
        "description": description,
        "args_schema_json": args_schema_json,
        "mapper": tool_mapper(mapper) if isinstance(mapper, str) else dict(mapper),
        "executor": dict(executor) if executor is not None else tool_executor_host_loop(),
        "parallelism_hint": {
            "parallel_safe": parallel_safe,
            "resource_key": resource_key,
        },
    }


def session_opened(config: Optional[Mapping[str, Any]] = None) -> dict[str, Any]:
    return _variant("SessionOpened", {"config": dict(config) if config is not None else None})


def run_completed() -> dict[str, Any]:
    return _unit_variant("RunCompleted")


def run_failed(code: str, detail: str) -> dict[str, Any]:
    return _variant("RunFailed", {"code": code, "detail": detail})


def run_cancelled(reason: Optional[str] = None) -> dict[str, Any]:
    return _variant("RunCancelled", {"reason": reason})


def direct_run_cause(
    input_ref: str,
    *,
    kind: str = "aos.agent/user_input",
    source: str = "aos.agent/RunRequested",
    request_ref: Optional[str] = None,
    payload_schema: Optional[str] = None,
    payload_ref: Optional[str] = None,
    subject_refs: Optional[Sequence[Mapping[str, Any]]] = None,
) -> dict[str, Any]:
    return {
        "kind": kind,
        "origin": _variant(
            "DirectIngress",
            {
                "source": source,
                "request_ref": request_ref,
            },
        ),
        "input_refs": [input_ref],
        "payload_schema": payload_schema,
        "payload_ref": payload_ref,
        "subject_refs": [dict(ref) for ref in subject_refs or []],
    }


def domain_event_run_cause(
    *,
    schema: str,
    event_ref: Optional[str] = None,
    key: Optional[str] = None,
    kind: str = "aos.agent/domain_event",
    input_refs: Optional[Sequence[str]] = None,
    payload_schema: Optional[str] = None,
    payload_ref: Optional[str] = None,
    subject_refs: Optional[Sequence[Mapping[str, Any]]] = None,
) -> dict[str, Any]:
    return {
        "kind": kind,
        "origin": _variant(
            "DomainEvent",
            {
                "schema": schema,
                "event_ref": event_ref,
                "key": key,
            },
        ),
        "input_refs": list(input_refs or []),
        "payload_schema": payload_schema,
        "payload_ref": payload_ref,
        "subject_refs": [dict(ref) for ref in subject_refs or []],
    }


def cause_ref(kind: str, id: str, ref: Optional[str] = None) -> dict[str, Any]:
    return {"kind": kind, "id": id, "ref_": ref}


def find_effect(
    effects: Iterable[Mapping[str, Any]],
    effect: str,
    *,
    predicate: Optional[Callable[[Mapping[str, Any]], bool]] = None,
    index: int = 0,
) -> dict[str, Any]:
    matches = [
        dict(item)
        for item in effects
        if item.get("effect") == effect and (predicate is None or predicate(item))
    ]
    if index < 0:
        index = len(matches) + index
    if index < 0 or index >= len(matches):
        raise AssertionError(f"expected effect {effect!r} at index {index}, found {len(matches)}")
    return matches[index]


def expect_llm_generate(
    effects: Iterable[Mapping[str, Any]],
    *,
    provider: Optional[str] = None,
    model: Optional[str] = None,
    message_refs: Optional[Sequence[str]] = None,
    index: int = 0,
) -> dict[str, Any]:
    def matches(effect: Mapping[str, Any]) -> bool:
        params = _effect_params(effect)
        if provider is not None and params.get("provider") != provider:
            return False
        if model is not None and params.get("model") != model:
            return False
        if message_refs is not None and params.get("message_refs") != list(message_refs):
            return False
        return True

    return find_effect(effects, LLM_GENERATE, predicate=matches, index=index)


def llm_output_envelope_bytes(
    *,
    assistant_text: Optional[str] = None,
    tool_calls_ref: Optional[str] = None,
    reasoning_ref: Optional[str] = None,
) -> bytes:
    return json.dumps(
        {
            "assistant_text": assistant_text,
            "tool_calls_ref": tool_calls_ref,
            "reasoning_ref": reasoning_ref,
        },
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")


def llm_tool_calls_bytes(tool_calls: Sequence[Mapping[str, Any]]) -> bytes:
    return json.dumps(
        [dict(call) for call in tool_calls],
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")


def llm_tool_call(
    *,
    call_id: str,
    tool_name: str,
    arguments_ref: str,
    provider_call_id: Optional[str] = None,
) -> dict[str, Any]:
    return {
        "call_id": call_id,
        "tool_name": tool_name,
        "arguments_ref": arguments_ref,
        "provider_call_id": provider_call_id,
    }


def apply_llm_generate_ok(
    harness: "WorkflowHarness",
    effect: Mapping[str, Any],
    *,
    output_ref: str,
    provider_id: str = "test-provider",
    finish_reason: str = "stop",
    prompt_tokens: int = 0,
    completion_tokens: int = 0,
    total_tokens: Optional[int] = None,
    raw_output_ref: Optional[str] = None,
    provider_response_id: Optional[str] = None,
    cost_cents: Optional[int] = None,
    warnings_ref: Optional[str] = None,
    rate_limit_ref: Optional[str] = None,
) -> ReceiptObject:
    receipt = llm_generate_ok(
        harness,
        effect,
        output_ref=output_ref,
        provider_id=provider_id,
        finish_reason=finish_reason,
        prompt_tokens=prompt_tokens,
        completion_tokens=completion_tokens,
        total_tokens=total_tokens,
        raw_output_ref=raw_output_ref,
        provider_response_id=provider_response_id,
        cost_cents=cost_cents,
        warnings_ref=warnings_ref,
        rate_limit_ref=rate_limit_ref,
    )
    harness.apply_receipt_object(receipt)
    return receipt


def respond_blob_get_bytes(
    harness: "WorkflowHarness",
    effect: Mapping[str, Any],
    data: bytes,
    *,
    blob_ref: Optional[str] = None,
    size: Optional[int] = None,
) -> ReceiptObject:
    params = _effect_params(effect)
    resolved_blob_ref = blob_ref or params.get("blob_ref")
    if not isinstance(resolved_blob_ref, str):
        raise TypeError("blob_ref must be supplied or present in effect.params.blob_ref")
    receipt = blob_get_ok(
        harness,
        effect,
        blob_ref=resolved_blob_ref,
        data=data,
        size=len(data) if size is None else size,
    )
    harness.apply_receipt_object(receipt)
    return receipt


def respond_llm_output_blob(
    harness: "WorkflowHarness",
    effect: Mapping[str, Any],
    *,
    assistant_text: Optional[str] = None,
    tool_calls_ref: Optional[str] = None,
    reasoning_ref: Optional[str] = None,
    blob_ref: Optional[str] = None,
) -> ReceiptObject:
    return respond_blob_get_bytes(
        harness,
        effect,
        llm_output_envelope_bytes(
            assistant_text=assistant_text,
            tool_calls_ref=tool_calls_ref,
            reasoning_ref=reasoning_ref,
        ),
        blob_ref=blob_ref,
    )


def respond_llm_tool_calls_blob(
    harness: "WorkflowHarness",
    effect: Mapping[str, Any],
    tool_calls: Sequence[Mapping[str, Any]],
    *,
    blob_ref: Optional[str] = None,
) -> ReceiptObject:
    return respond_blob_get_bytes(
        harness,
        effect,
        llm_tool_calls_bytes(tool_calls),
        blob_ref=blob_ref,
    )


def session_state(harness: "WorkflowHarness", session_id: Optional[str] = None) -> Any:
    if session_id is not None:
        state = harness.state_get(session_key(session_id))
        if state is not None:
            return state

    cells = harness.list_cells()
    if not cells:
        return None
    if session_id is None and len(cells) == 1:
        return harness.state_get(bytes(cells[0]["key_bytes"]))
    raise KeyError(f"session state not found for {session_id!r}; cells={len(cells)}")


def current_run(state_or_harness: Any, session_id: Optional[str] = None) -> Optional[dict[str, Any]]:
    state = (
        session_state(state_or_harness, session_id)
        if hasattr(state_or_harness, "state_get")
        else state_or_harness
    )
    if not isinstance(state, Mapping):
        return None
    run = state.get("current_run")
    return dict(run) if isinstance(run, Mapping) else None


def current_turn_plan(
    state_or_harness: Any,
    session_id: Optional[str] = None,
) -> Optional[dict[str, Any]]:
    run = current_run(state_or_harness, session_id)
    if run is None:
        return None
    plan = run.get("turn_plan")
    return dict(plan) if isinstance(plan, Mapping) else None


def selected_message_refs(state_or_plan: Any, session_id: Optional[str] = None) -> list[str]:
    plan = (
        state_or_plan
        if isinstance(state_or_plan, Mapping) and "message_refs" in state_or_plan
        else current_turn_plan(state_or_plan, session_id)
    )
    if not isinstance(plan, Mapping):
        return []
    return [str(ref) for ref in plan.get("message_refs", [])]


def selected_tool_ids(state_or_plan: Any, session_id: Optional[str] = None) -> list[str]:
    plan = (
        state_or_plan
        if isinstance(state_or_plan, Mapping) and "selected_tool_ids" in state_or_plan
        else current_turn_plan(state_or_plan, session_id)
    )
    if not isinstance(plan, Mapping):
        return []
    return [str(tool_id) for tool_id in plan.get("selected_tool_ids", [])]


def run_trace_entries(state_or_run: Any, session_id: Optional[str] = None) -> list[dict[str, Any]]:
    run = (
        state_or_run
        if isinstance(state_or_run, Mapping) and "trace" in state_or_run
        else current_run(state_or_run, session_id)
    )
    if not isinstance(run, Mapping):
        return []
    trace = run.get("trace")
    if not isinstance(trace, Mapping):
        return []
    return [dict(entry) for entry in trace.get("entries", []) if isinstance(entry, Mapping)]


def run_trace_kinds(state_or_run: Any, session_id: Optional[str] = None) -> list[str]:
    return [_tag(entry.get("kind")) for entry in run_trace_entries(state_or_run, session_id)]


def last_trace_kind(state_or_run: Any, session_id: Optional[str] = None) -> Optional[str]:
    kinds = run_trace_kinds(state_or_run, session_id)
    return kinds[-1] if kinds else None


def trace_contains_kind(state_or_run: Any, kind: str, session_id: Optional[str] = None) -> bool:
    return kind in run_trace_kinds(state_or_run, session_id)


def require_trace_kinds(
    state_or_run: Any,
    kinds: Sequence[str],
    session_id: Optional[str] = None,
) -> None:
    actual = run_trace_kinds(state_or_run, session_id)
    missing = [kind for kind in kinds if kind not in actual]
    if missing:
        raise AssertionError(f"missing trace kinds {missing}; actual={actual}")


def run_history_summaries(
    state_or_harness: Any,
    session_id: Optional[str] = None,
) -> list[dict[str, Any]]:
    state = (
        session_state(state_or_harness, session_id)
        if hasattr(state_or_harness, "state_get")
        else state_or_harness
    )
    if not isinstance(state, Mapping):
        return []
    history = state.get("run_history", [])
    if not isinstance(history, Sequence):
        return []
    summaries = []
    for record in history:
        if not isinstance(record, Mapping):
            continue
        summaries.append(
            {
                "run_id": record.get("run_id"),
                "lifecycle": record.get("lifecycle"),
                "outcome": record.get("outcome"),
                "last_llm_usage": record.get("last_llm_usage"),
                "trace_summary": record.get("trace_summary"),
                "started_at": record.get("started_at"),
                "ended_at": record.get("ended_at"),
            }
        )
    return summaries


def last_llm_usage(state_or_run: Any, session_id: Optional[str] = None) -> Optional[dict[str, Any]]:
    run = (
        state_or_run
        if isinstance(state_or_run, Mapping) and "last_llm_usage" in state_or_run
        else current_run(state_or_run, session_id)
    )
    if isinstance(run, Mapping) and isinstance(run.get("last_llm_usage"), Mapping):
        return dict(run["last_llm_usage"])
    history = state_or_run.get("run_history") if isinstance(state_or_run, Mapping) else None
    if isinstance(history, Sequence) and history:
        last = history[-1]
        if isinstance(last, Mapping) and isinstance(last.get("last_llm_usage"), Mapping):
            return dict(last["last_llm_usage"])
    return None


__all__ = [
    "BLOB_GET",
    "LLM_GENERATE",
    "SESSION_INPUT",
    "SESSION_WORKFLOW",
    "SESSION_WORKFLOW_EVENT",
    "agent_workflow",
    "apply_llm_generate_ok",
    "cause_ref",
    "current_run",
    "current_turn_plan",
    "direct_run_cause",
    "domain_event_run_cause",
    "expect_llm_generate",
    "find_effect",
    "follow_up_input_appended",
    "host_session_updated",
    "last_llm_usage",
    "last_trace_kind",
    "llm_output_envelope_bytes",
    "llm_tool_call",
    "llm_tool_calls_bytes",
    "respond_blob_get_bytes",
    "respond_llm_output_blob",
    "respond_llm_tool_calls_blob",
    "require_trace_kinds",
    "run_cancelled",
    "run_completed",
    "run_failed",
    "run_history_summaries",
    "run_interrupt_requested",
    "run_requested",
    "run_start_requested",
    "run_steer_requested",
    "run_trace_entries",
    "run_trace_kinds",
    "selected_message_refs",
    "selected_tool_ids",
    "send_session_input",
    "session_config",
    "session_input",
    "session_key",
    "session_opened",
    "session_state",
    "tool_executor_domain_event",
    "tool_executor_effect",
    "tool_executor_host_loop",
    "tool_mapper",
    "tool_profile_selected",
    "tool_registry_set",
    "tool_spec",
    "trace_contains_kind",
    "turn_observed",
]
