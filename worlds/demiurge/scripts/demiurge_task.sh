#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORLD_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_DIR="$(cd "${WORLD_DIR}/../.." && pwd)"

AOS_TIMEOUT_MS="${AOS_TIMEOUT_MS:-180000}"
POLL_INTERVAL_SEC="${POLL_INTERVAL_SEC:-1}"

TASK_ID=""
TASK_TEXT=""
WORKDIR_VALUE="${REPO_DIR}"
PROVIDER="openai-responses"
MODEL="gpt-5.3-codex"
MAX_TOKENS=100000
TOOL_PROFILE="openai"
ALLOWED_TOOLS_CSV=""
TOOL_ENABLE_CSV=""
TOOL_DISABLE_CSV=""
TOOL_FORCE_CSV=""

usage() {
  cat <<'USAGE'
Usage:
  worlds/demiurge/scripts/demiurge_task.sh --task "..."

Options:
  --task <text>              Required task prompt.
  --task-id <uuid>           Optional task/session id (default: random uuid4).
  --workdir <abs-path>       Workdir passed to Demiurge (default: repo root).
  --provider <name>          LLM provider (default: openai-responses).
  --model <name>             LLM model (default: gpt-5.3-codex).
  --max-tokens <n>           Max completion tokens (default: 100000).
  --tool-profile <id>        Tool profile (default: openai).
  --allowed-tools <csv>      Optional allowlist (tool ids).
  --tool-enable <csv>        Optional enabled tools (tool ids).
  --tool-disable <csv>       Optional disabled tools (tool ids).
  --tool-force <csv>         Optional forced tools (tool ids).
  -h, --help                 Show this help.

Notes:
  - Uses the currently selected CLI profile, universe, and world.
  - The selected world must already be a Demiurge world.
  - This script does not modify profile selection.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --task)
      TASK_TEXT="${2:-}"
      shift 2
      ;;
    --task-id)
      TASK_ID="${2:-}"
      shift 2
      ;;
    --workdir)
      WORKDIR_VALUE="${2:-}"
      shift 2
      ;;
    --provider)
      PROVIDER="${2:-}"
      shift 2
      ;;
    --model)
      MODEL="${2:-}"
      shift 2
      ;;
    --max-tokens)
      MAX_TOKENS="${2:-}"
      shift 2
      ;;
    --tool-profile)
      TOOL_PROFILE="${2:-}"
      shift 2
      ;;
    --allowed-tools)
      ALLOWED_TOOLS_CSV="${2:-}"
      shift 2
      ;;
    --tool-enable)
      TOOL_ENABLE_CSV="${2:-}"
      shift 2
      ;;
    --tool-disable)
      TOOL_DISABLE_CSV="${2:-}"
      shift 2
      ;;
    --tool-force)
      TOOL_FORCE_CSV="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "${TASK_TEXT}" ]]; then
  echo "--task is required" >&2
  usage >&2
  exit 2
fi

if [[ -z "${TASK_ID}" ]]; then
  TASK_ID="$(python3 - <<'PY'
import uuid
print(uuid.uuid4())
PY
)"
fi

if [[ ! "${WORKDIR_VALUE}" = /* ]]; then
  echo "--workdir must be an absolute path: ${WORKDIR_VALUE}" >&2
  exit 2
fi

AOS_BIN="${REPO_DIR}/target/debug/aos"
if [[ ! -x "${AOS_BIN}" ]]; then
  AOS_BIN="aos"
fi

RESULT_FILE="$(mktemp -t demiurge-result-XXXX.json)"
cleanup() {
  rm -f "${RESULT_FILE}"
}
trap cleanup EXIT

if ! "${AOS_BIN}" --json --quiet world status >/dev/null; then
  echo "current CLI target must resolve to a selected universe and world" >&2
  echo "use \`aos universe create --select\`, \`aos world create --select\`, or explicit --universe/--world overrides" >&2
  exit 1
fi

TASK_EVENT="$(python3 - <<'PY' \
  "${TASK_ID}" "${WORKDIR_VALUE}" "${TASK_TEXT}" \
  "${PROVIDER}" "${MODEL}" "${MAX_TOKENS}" "${TOOL_PROFILE}" \
  "${ALLOWED_TOOLS_CSV}" "${TOOL_ENABLE_CSV}" "${TOOL_DISABLE_CSV}" "${TOOL_FORCE_CSV}"
import json
import sys

def csv_to_list(text):
    values = [item.strip() for item in text.split(",") if item.strip()]
    return values if values else None

task_id = sys.argv[1]
workdir = sys.argv[2]
task = sys.argv[3]
provider = sys.argv[4]
model = sys.argv[5]
max_tokens = int(sys.argv[6])
tool_profile = sys.argv[7]
allowed_tools = csv_to_list(sys.argv[8])
tool_enable = csv_to_list(sys.argv[9])
tool_disable = csv_to_list(sys.argv[10])
tool_force = csv_to_list(sys.argv[11])

payload = {
    "task_id": task_id,
    "observed_at_ns": 1,
    "workdir": workdir,
    "task": task,
    "config": {
        "provider": provider,
        "model": model,
        "reasoning_effort": None,
        "max_tokens": max_tokens,
        "tool_profile": tool_profile,
        "allowed_tools": allowed_tools,
        "tool_enable": tool_enable,
        "tool_disable": tool_disable,
        "tool_force": tool_force,
        "session_ttl_ns": None,
    },
}
print(json.dumps(payload, separators=(",", ":")))
PY
)"

echo "submitting task_id=${TASK_ID}"
"${AOS_BIN}" --json --quiet \
  world send \
  --schema "demiurge/TaskSubmitted@1" \
  --value-json "${TASK_EVENT}" >"${RESULT_FILE}"

python3 - <<'PY' "${RESULT_FILE}" "${AOS_BIN}" "${TASK_ID}" "${AOS_TIMEOUT_MS}" "${POLL_INTERVAL_SEC}"
import base64
import json
import subprocess
import sys
import time

result_doc = json.load(open(sys.argv[1], "r", encoding="utf-8"))
aos_bin, task_id, timeout_ms, poll_interval = sys.argv[2:6]
timeout_ms = int(timeout_ms)
poll_interval = max(float(poll_interval), 0.1)
event_hash = ((result_doc.get("data") or {}).get("event_hash"))

def get_state(workflow):
    try:
        raw = subprocess.check_output(
            [
                aos_bin,
                "--json",
                "--quiet",
                "world",
                "state",
                "get",
                workflow,
                task_id,
                "--expand",
            ],
            stderr=subprocess.DEVNULL,
        ).decode("utf-8", errors="replace")
    except subprocess.CalledProcessError:
        return {}
    doc = json.loads(raw)
    return ((doc.get("data") or {}).get("state_expanded")) or {}

deadline = time.time() + (timeout_ms / 1000.0)
state = {}
session = {}
while time.time() < deadline:
    state = get_state("demiurge/Demiurge@1")
    session = get_state("aos.agent/SessionWorkflow@1")
    task_finished = bool(state.get("finished"))
    task_failure = state.get("failure")
    if task_failure:
        break
    if task_finished and session:
        break
    time.sleep(poll_interval)

task_status = ((state.get("status") or {}).get("$tag")) or "Unknown"
task_finished = bool(state.get("finished"))
task_failure = state.get("failure")
host_session_id = state.get("host_session_id")
run_id = ((session.get("active_run_id") or {}).get("run_seq"))
session_lifecycle = ((session.get("lifecycle") or {}).get("$tag")) or "Unknown"
terminal_state = "completed" if task_finished else "pending"
output_ref = state.get("output_ref") or session.get("last_output_ref")
assistant_text = None
extract_error = None

if assistant_text is None and output_ref:
    try:
        blob_doc = json.loads(
            subprocess.check_output(
                [
                    aos_bin,
                    "--json",
                    "--quiet",
                    "cas",
                    "get",
                    str(output_ref),
                ],
                stderr=subprocess.DEVNULL,
            ).decode("utf-8", errors="replace")
        )
        data_b64 = ((blob_doc.get("data") or {}).get("data_b64")) or ""
        if data_b64:
            assistant_text = json.loads(
                base64.b64decode(data_b64).decode("utf-8", errors="replace")
            ).get("assistant_text")
    except Exception as exc:
        extract_error = f"read llm output blob failed: {exc}"

failure_exit = None
if not isinstance(state, dict) or not state:
    failure_exit = "missing demiurge keyed state"
elif not isinstance(session, dict) or not session:
    failure_exit = "missing session workflow keyed state"
elif not host_session_id:
    failure_exit = "missing host_session_id"
elif task_failure:
    failure_exit = f"task failure={json.dumps(task_failure, sort_keys=True)}"
elif not task_finished:
    failure_exit = f"task did not finish before timeout terminal_state={terminal_state}"

print("")
print(f"task_id: {task_id}")
print(f"event_hash: {event_hash}")
print(f"task_status: {task_status} finished={task_finished}")
print(f"session_lifecycle: {session_lifecycle}")
print(f"terminal_state: {terminal_state}")
print(f"host_session_id: {host_session_id}")
if run_id is not None:
    print(f"run_seq: {run_id}")

if task_failure:
    print(
        f"task_failure: code={task_failure.get('code')} detail={task_failure.get('detail')}"
    )

print("")
print("final_assistant_response:")
if assistant_text is not None and str(assistant_text).strip():
    print(str(assistant_text))
elif output_ref:
    print(f"<assistant_text missing; output_ref={output_ref}>")
else:
    print("<not found>")

if extract_error:
    print("")
    print(f"response_extract_note: {extract_error}")

if failure_exit:
    raise SystemExit(failure_exit)
PY
