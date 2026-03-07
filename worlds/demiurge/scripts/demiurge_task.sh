#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORLD_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_DIR="$(cd "${WORLD_DIR}/../.." && pwd)"

AOS_TIMEOUT_MS="${AOS_TIMEOUT_MS:-180000}"
POLL_INTERVAL_SEC="${POLL_INTERVAL_SEC:-1}"
MAX_POLLS="${MAX_POLLS:-600}"
TRACE_WINDOW_LIMIT="${TRACE_WINDOW_LIMIT:-1200}"
API_BASE_URL="${AOS_API_BASE_URL:-http://127.0.0.1:7777}"

TASK_ID=""
TASK_TEXT=""
WORKDIR_VALUE="${REPO_DIR}"
PROVIDER="openai-responses"
MODEL="gpt-5.3-codex"
MAX_TOKENS=4096
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
  --max-tokens <n>           Max completion tokens (default: 4096).
  --tool-profile <id>        Tool profile (default: openai).
  --allowed-tools <csv>      Optional allowlist (tool ids).
  --tool-enable <csv>        Optional enabled tools (tool ids).
  --tool-disable <csv>       Optional disabled tools (tool ids).
  --tool-force <csv>         Optional forced tools (tool ids).
  --api-base <url>           Daemon HTTP API base (default: http://127.0.0.1:7777).
  -h, --help                 Show this help.

Notes:
  - Requires the Demiurge daemon to be running first.
  - Run worlds/demiurge/scripts/demiurge_up.sh --run in another terminal.
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
    --api-base)
      API_BASE_URL="${2:-}"
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

if ! command -v curl >/dev/null 2>&1; then
  echo "curl is required for /api/events submission" >&2
  exit 1
fi

AOS_BIN="${REPO_DIR}/target/debug/aos"
if [[ ! -x "${AOS_BIN}" ]]; then
  AOS_BIN="aos"
fi

STATUS_FILE="$(mktemp -t demiurge-status-XXXX.json)"
STATE_FILE="$(mktemp -t demiurge-state-XXXX.json)"
SESSION_FILE="$(mktemp -t demiurge-session-XXXX.json)"
TRACE_FILE="$(mktemp -t demiurge-trace-XXXX.json)"
cleanup() {
  rm -f "${STATUS_FILE}" "${STATE_FILE}" "${SESSION_FILE}" "${TRACE_FILE}"
}
trap cleanup EXIT

"${AOS_BIN}" --timeout-ms "${AOS_TIMEOUT_MS}" --json --quiet -w "${WORLD_DIR}" status >"${STATUS_FILE}"

python3 - <<'PY' "${STATUS_FILE}"
import json
import sys

doc = json.load(open(sys.argv[1], "r", encoding="utf-8"))
running = (((doc.get("data") or {}).get("daemon") or {}).get("running"))
if running is not True:
    raise SystemExit(
        "demiurge daemon is not running; start it with worlds/demiurge/scripts/demiurge_up.sh --run"
    )
PY

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
EVENT_POST_BODY="$(python3 - <<'PY' "${TASK_EVENT}"
import json
import sys

value = json.loads(sys.argv[1])
payload = {
    "schema": "demiurge/TaskSubmitted@1",
    "value": value,
}
print(json.dumps(payload, separators=(",", ":")))
PY
)"
curl -fsS \
  -H "content-type: application/json" \
  -X POST \
  --data "${EVENT_POST_BODY}" \
  "${API_BASE_URL}/api/events" >/dev/null

poll=0
done_flag=0
while (( poll < MAX_POLLS )); do
  "${AOS_BIN}" --timeout-ms "${AOS_TIMEOUT_MS}" --json --quiet -w "${WORLD_DIR}" \
    state get "demiurge/Demiurge@1" --key "${TASK_ID}" >"${STATE_FILE}"
  done_flag="$(python3 - <<'PY' "${STATE_FILE}"
import json
import sys

doc = json.load(open(sys.argv[1], "r", encoding="utf-8"))
state = doc.get("data") or {}
status = ((state.get("status") or {}).get("$tag")) or "Unknown"
finished = bool(state.get("finished"))
failure = state.get("failure")
if failure:
    code = failure.get("code")
    detail = failure.get("detail")
    print(f"{int(finished)}|{status}|{code}|{detail}")
else:
    print(f"{int(finished)}|{status}||")
PY
)"
  IFS='|' read -r finished status fail_code fail_detail <<<"${done_flag}"
  echo "poll=${poll} status=${status} finished=${finished}"
  if [[ "${finished}" == "1" ]]; then
    break
  fi
  sleep "${POLL_INTERVAL_SEC}"
  poll=$((poll + 1))
done

"${AOS_BIN}" --timeout-ms "${AOS_TIMEOUT_MS}" --json --quiet -w "${WORLD_DIR}" \
  state get "demiurge/Demiurge@1" --key "${TASK_ID}" >"${STATE_FILE}"
"${AOS_BIN}" --timeout-ms "${AOS_TIMEOUT_MS}" --json --quiet -w "${WORLD_DIR}" \
  state get "aos.agent/SessionWorkflow@1" --key "${TASK_ID}" >"${SESSION_FILE}"
"${AOS_BIN}" --timeout-ms "${AOS_TIMEOUT_MS}" --json --quiet -w "${WORLD_DIR}" \
  trace --schema "demiurge/TaskSubmitted@1" --correlate-by "task_id" --value "\"${TASK_ID}\"" \
  --window-limit "${TRACE_WINDOW_LIMIT}" >"${TRACE_FILE}"

python3 - <<'PY' "${STATE_FILE}" "${SESSION_FILE}" "${TRACE_FILE}" "${AOS_BIN}" "${WORLD_DIR}" "${TASK_ID}"
import json
import subprocess
import sys

state_path, session_path, trace_path, aos_bin, world_dir, task_id = sys.argv[1:7]

def load_json(path):
    with open(path, "r", encoding="utf-8") as fh:
        return json.load(fh)

def read_uint(data, idx, ai):
    if ai < 24:
        return ai, idx
    if ai == 24:
        return data[idx], idx + 1
    if ai == 25:
        return int.from_bytes(data[idx:idx + 2], "big"), idx + 2
    if ai == 26:
        return int.from_bytes(data[idx:idx + 4], "big"), idx + 4
    if ai == 27:
        return int.from_bytes(data[idx:idx + 8], "big"), idx + 8
    raise ValueError("unsupported additional-info")

def decode_cbor(data):
    def dec(idx):
        head = data[idx]
        idx += 1
        major = head >> 5
        ai = head & 0x1F
        if major in (0, 1):
            value, idx = read_uint(data, idx, ai)
            return (value if major == 0 else -1 - value), idx
        if major == 2:
            length, idx = read_uint(data, idx, ai)
            out = bytes(data[idx:idx + length])
            return out, idx + length
        if major == 3:
            length, idx = read_uint(data, idx, ai)
            out = bytes(data[idx:idx + length]).decode("utf-8", errors="replace")
            return out, idx + length
        if major == 4:
            length, idx = read_uint(data, idx, ai)
            arr = []
            for _ in range(length):
                v, idx = dec(idx)
                arr.append(v)
            return arr, idx
        if major == 5:
            length, idx = read_uint(data, idx, ai)
            m = {}
            for _ in range(length):
                key, idx = dec(idx)
                val, idx = dec(idx)
                m[key] = val
            return m, idx
        if major == 6:
            _, idx = read_uint(data, idx, ai)
            return dec(idx)
        if major == 7:
            if ai == 20:
                return False, idx
            if ai == 21:
                return True, idx
            if ai == 22:
                return None, idx
            if ai == 23:
                return None, idx
            raise ValueError("unsupported simple value")
        raise ValueError("unsupported major type")

    value, end = dec(0)
    if end != len(data):
        return value
    return value

state_doc = load_json(state_path)
session_doc = load_json(session_path)
trace_doc = load_json(trace_path)

state = state_doc.get("data") or {}
session = session_doc.get("data") or {}
trace = trace_doc.get("data") or {}

task_status = ((state.get("status") or {}).get("$tag")) or "Unknown"
task_finished = bool(state.get("finished"))
task_failure = state.get("failure")
host_session_id = state.get("host_session_id")
run_id = ((session.get("active_run_id") or {}).get("run_seq"))
session_lifecycle = ((session.get("lifecycle") or {}).get("$tag")) or "Unknown"
event_hash = ((trace.get("root") or {}).get("event_hash"))
terminal_state = trace.get("terminal_state")

entries = ((trace.get("journal_window") or {}).get("entries") or [])
llm_intents = {}
candidate_receipt = None
candidate_seq = -1
for item in entries:
    kind = item.get("kind")
    seq = int(item.get("seq") or 0)
    record = item.get("record") or {}
    if kind == "effect_intent" and record.get("kind") == "llm.generate":
        llm_intents[tuple(record.get("intent_hash") or [])] = seq
    if kind == "effect_receipt" and record.get("status") == "ok":
        intent_hash = tuple(record.get("intent_hash") or [])
        if intent_hash in llm_intents and seq >= candidate_seq:
            candidate_seq = seq
            candidate_receipt = record

output_ref = None
assistant_text = None
extract_error = None
if candidate_receipt:
    payload_bytes = bytes(candidate_receipt.get("payload_cbor") or [])
    try:
        payload = decode_cbor(payload_bytes)
        if isinstance(payload, dict):
            output_ref = payload.get("output_ref")
    except Exception as exc:
        extract_error = f"decode llm receipt failed: {exc}"

if output_ref:
    try:
        blob_bytes = subprocess.check_output(
            [aos_bin, "--quiet", "-w", world_dir, "blob", "get", "--raw", str(output_ref)],
            stderr=subprocess.STDOUT,
        )
        envelope = json.loads(blob_bytes.decode("utf-8", errors="replace"))
        assistant_text = envelope.get("assistant_text")
    except Exception as exc:
        extract_error = f"read llm output blob failed: {exc}"

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
PY
