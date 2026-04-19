import json
import os
import time

from aos_harness import WorkflowHarness
from aos_harness.testing import smoke_fixture_root


EVENT_SCHEMA = "demo/TimerEvent@1"
WORKFLOW_NAME = "demo/TimerSM@1"
DELIVER_AT_NS = 1_000_000
VERBOSE = os.environ.get("AOS_HARNESS_VERBOSE", "").lower() not in {"", "0", "false", "no"}


def runtime_quiescent(status: dict) -> bool:
    return bool(status.get("runtime_quiescent", False))


def next_timer_deadline(status: dict) -> int | None:
    deadline = status.get("next_timer_deadline_ns")
    return int(deadline) if deadline is not None else None


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
    fixture_root = smoke_fixture_root("01-hello-timer")
    log.log(f"fixture root: {fixture_root}")
    log.log("opening workflow harness from authored workflow (may compile/build on first run)")
    harness = WorkflowHarness.from_workflow_dir(
        WORKFLOW_NAME,
        str(fixture_root / "workflow"),
        effect_mode="scripted",
        build_profile="release",
    )
    log.log("workflow harness ready")

    log.log(f"sending event {EVENT_SCHEMA}")
    harness.send_event(
        EVENT_SCHEMA,
        {"Start": {"deliver_at_ns": DELIVER_AT_NS, "key": "retry"}},
    )

    rounds = 0
    while True:
        rounds += 1
        log.log(f"round {rounds}: run_to_idle")
        status = harness.run_to_idle()
        log.log(f"round {rounds}: quiescence={status}")
        if runtime_quiescent(status):
            log.log(f"round {rounds}: runtime quiescent")
            break

        deadline = next_timer_deadline(status)
        if deadline is None:
            raise AssertionError(f"not quiescent and no pending timer: {status}")

        log.log(f"round {rounds}: jumping logical time to next timer at {deadline}")
        jumped_to = harness.time_jump_next_due()
        assert jumped_to == deadline

    log.log("reading state and exporting artifacts")
    state = harness.state_get()
    reopened = harness.reopen()
    reopened_state = reopened.state_get()
    artifacts = harness.artifact_export()

    assert rounds >= 1
    assert state["deadline_ns"] == DELIVER_AT_NS
    assert state["fired_key"] == "retry"
    assert reopened_state == state
    assert artifacts["evidence"]["cycles_run"] >= 1
    assert artifacts["journal_entries"], "expected journal entries in exported artifacts"

    print("timer_smoke.py: OK")
    print(json.dumps({"state": state, "evidence": artifacts["evidence"]}, indent=2))


if __name__ == "__main__":
    main()
