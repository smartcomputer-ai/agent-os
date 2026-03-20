#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORLD_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_DIR="$(cd "${WORLD_DIR}/../.." && pwd)"

AOS_TIMEOUT_MS="${AOS_TIMEOUT_MS:-180000}"
AOS_POLL_INTERVAL_SEC="${AOS_POLL_INTERVAL_SEC:-1}"
LOCAL_WORLD_HANDLE="${AOS_LOCAL_WORLD_HANDLE:-demiurge}"

TASK_ID="33333333-3333-3333-3333-333333333333"
TASK_TEXT="Read README.md and summarize the project name in one sentence."
WORKDIR_VALUE="${REPO_DIR}"
PROVIDER="${AOS_DEMIURGE_PROVIDER:-}"
MODEL="${AOS_DEMIURGE_MODEL:-}"
MAX_TOKENS="${AOS_DEMIURGE_MAX_TOKENS:-256}"
TOOL_PROFILE="${AOS_DEMIURGE_TOOL_PROFILE:-openai}"
ALLOWED_TOOLS_CSV="${AOS_DEMIURGE_ALLOWED_TOOLS:-host.fs.read_file}"
TOOL_ENABLE_CSV="${AOS_DEMIURGE_TOOL_ENABLE:-host.fs.read_file}"

usage() {
  cat <<'USAGE'
Usage:
  worlds/demiurge/scripts/smoke_task_submit.sh [options]

Options:
  --task <text>           Task prompt to submit.
  --task-id <uuid>        Task/session id to use.
  --workdir <abs-path>    Absolute workdir passed to Demiurge.
  --provider <id>         LLM provider id (default: mock).
  --model <id>            Model id (default: gpt-5.3-codex).
  --max-tokens <n>        Max completion tokens (default: 256).
  --tool-profile <id>     Tool profile (default: openai).
  --allowed-tools <csv>   Allowed tools CSV (default: host.fs.read_file).
  --tool-enable <csv>     Enabled tools CSV (default: host.fs.read_file).
  -h, --help              Show help.

Notes:
  - This is the current local-runtime smoke path.
  - It resets worlds/demiurge/.aos, starts `aos local` against that root,
    creates a fresh local universe/world, submits a task, and verifies state.
  - Provider selection defaults to `openai-responses` when `OPENAI_API_KEY` is
    present, `anthropic` when `ANTHROPIC_API_KEY` is present, and `mock`
    otherwise.
  - With the `mock` provider, success means Demiurge and SessionWorkflow both
    start correctly in the local runtime.
  - With a live provider, the script waits for terminal completion.
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

if [[ ! "${WORKDIR_VALUE}" = /* ]]; then
  echo "--workdir must be an absolute path: ${WORKDIR_VALUE}" >&2
  exit 2
fi

if [[ -f "${WORLD_DIR}/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "${WORLD_DIR}/.env"
  set +a
fi

if [[ -z "${PROVIDER}" ]]; then
  if [[ -n "${OPENAI_API_KEY:-}" ]]; then
    PROVIDER="openai-responses"
  elif [[ -n "${ANTHROPIC_API_KEY:-}" ]]; then
    PROVIDER="anthropic"
  else
    PROVIDER="mock"
  fi
fi

if [[ -z "${MODEL}" ]]; then
  case "${PROVIDER}" in
    anthropic*)
      MODEL="claude-sonnet-4-5"
      ;;
    *)
      MODEL="gpt-5.3-codex"
      ;;
  esac
fi

csv_json() {
  python3 - <<'PY' "$1"
import json
import sys
values = [item.strip() for item in sys.argv[1].split(",") if item.strip()]
print(json.dumps(values if values else None, separators=(",", ":")))
PY
}

echo "building local demiurge prerequisites"
(
  cd "${REPO_DIR}"
  cargo build -p aos-cli -p aos-node-local >/dev/null
  cargo build -p aos-sys --target wasm32-unknown-unknown >/dev/null
  cargo build -p aos-agent --bin session_workflow --target wasm32-unknown-unknown >/dev/null
)

AOS_BIN="${REPO_DIR}/target/debug/aos"
SESSION_WASM="${REPO_DIR}/target/wasm32-unknown-unknown/debug/session_workflow.wasm"
SESSION_HASH="sha256:$(shasum -a 256 "${SESSION_WASM}" | awk '{print $1}')"
SESSION_MODULE_DIR="${WORLD_DIR}/modules/aos.agent"
SESSION_MODULE_PATH="${SESSION_MODULE_DIR}/SessionWorkflow@1-${SESSION_HASH}.wasm"

mkdir -p "${SESSION_MODULE_DIR}"
rm -f "${SESSION_MODULE_DIR}"/SessionWorkflow@1-*.wasm
cp "${SESSION_WASM}" "${SESSION_MODULE_PATH}"

echo "resetting local state root ${WORLD_DIR}/.aos"
"${AOS_BIN}" local down --root "${WORLD_DIR}" --force >/dev/null 2>&1 || true
rm -rf "${WORLD_DIR}/.aos"

echo "starting local node on world root ${WORLD_DIR}"
"${AOS_BIN}" local up --root "${WORLD_DIR}" --select >/dev/null
for _ in $(seq 1 20); do
  STATUS_JSON="$("${AOS_BIN}" --json --quiet local status --root "${WORLD_DIR}" || true)"
  if python3 - <<'PY' "${STATUS_JSON}"
import json
import sys
raw = sys.argv[1]
if not raw.strip():
    raise SystemExit(1)
doc = json.loads(raw)
data = doc.get("data") or {}
raise SystemExit(0 if data.get("healthy") is True else 1)
PY
  then
    break
  fi
  sleep 0.5
done

echo "using provider=${PROVIDER} model=${MODEL}"

if [[ "${PROVIDER}" == "mock" ]]; then
  :
elif [[ "${PROVIDER}" == openai* ]]; then
  if [[ -z "${OPENAI_API_KEY:-}" ]]; then
    echo "missing OPENAI_API_KEY for provider ${PROVIDER}" >&2
    exit 1
  fi
  echo "binding llm/openai_api to worker env OPENAI_API_KEY"
  "${AOS_BIN}" universe secret binding set "llm/openai_api" worker_env --env-var "OPENAI_API_KEY" >/dev/null
elif [[ "${PROVIDER}" == anthropic* ]]; then
  if [[ -z "${ANTHROPIC_API_KEY:-}" ]]; then
    echo "missing ANTHROPIC_API_KEY for provider ${PROVIDER}" >&2
    exit 1
  fi
  echo "binding llm/anthropic_api to worker env ANTHROPIC_API_KEY"
  "${AOS_BIN}" universe secret binding set "llm/anthropic_api" worker_env --env-var "ANTHROPIC_API_KEY" >/dev/null
fi

echo "creating local world ${LOCAL_WORLD_HANDLE} from ${WORLD_DIR}"
"${AOS_BIN}" world create --local-root "${WORLD_DIR}" --handle "${LOCAL_WORLD_HANDLE}" --force-build --select >/dev/null

ALLOWED_TOOLS_JSON="$(csv_json "${ALLOWED_TOOLS_CSV}")"
TOOL_ENABLE_JSON="$(csv_json "${TOOL_ENABLE_CSV}")"

TASK_EVENT="$(python3 - <<'PY' \
  "${TASK_ID}" "${WORKDIR_VALUE}" "${TASK_TEXT}" \
  "${PROVIDER}" "${MODEL}" "${MAX_TOKENS}" "${TOOL_PROFILE}" \
  "${ALLOWED_TOOLS_JSON}" "${TOOL_ENABLE_JSON}"
import json
import sys

payload = {
    "task_id": sys.argv[1],
    "observed_at_ns": 1,
    "workdir": sys.argv[2],
    "task": sys.argv[3],
    "config": {
        "provider": sys.argv[4],
        "model": sys.argv[5],
        "reasoning_effort": None,
        "max_tokens": int(sys.argv[6]),
        "tool_profile": sys.argv[7],
        "allowed_tools": json.loads(sys.argv[8]),
        "tool_enable": json.loads(sys.argv[9]),
        "tool_disable": None,
        "tool_force": None,
        "session_ttl_ns": None,
    },
}
print(json.dumps(payload, separators=(",", ":")))
PY
)"

echo "submitting demiurge task ${TASK_ID}"
RESULT_FILE="$(mktemp -t demiurge-local-result-XXXX.json)"
cleanup() {
  rm -f "${RESULT_FILE}"
}
trap cleanup EXIT

if ! "${AOS_BIN}" --json --quiet \
  world send \
  --schema "demiurge/TaskSubmitted@1" \
  --value-json "${TASK_EVENT}" \
  --follow \
  --correlate-by "task_id" \
  --correlate-value "\"${TASK_ID}\"" \
  --interval-ms "$((AOS_POLL_INTERVAL_SEC * 1000))" \
  --timeout-ms "${AOS_TIMEOUT_MS}" \
  --result-workflow "demiurge/Demiurge@1" \
  --result-workflow "aos.agent/SessionWorkflow@1" \
  --result-key "${TASK_ID}" \
  --result-expand \
  --blob-ref-workflow "demiurge/Demiurge@1" \
  --blob-ref-field "output_ref" \
  --blob-json-field "assistant_text" >"${RESULT_FILE}"; then
  echo "failed: aos world send --follow did not complete successfully" >&2
  exit 1
fi

python3 - <<'PY' "${RESULT_FILE}" "${TASK_ID}" "${PROVIDER}" "${AOS_BIN}"
import json
import base64
import subprocess
import sys

result_doc = json.load(open(sys.argv[1], "r", encoding="utf-8"))
task_id = sys.argv[2]
provider = sys.argv[3]
aos_bin = sys.argv[4]

result = result_doc.get("data") or {}
states = result.get("states") or {}
trace = result.get("trace") or {}
state = (states.get("demiurge/Demiurge@1") or {}).get("state_expanded") or {}
session = (states.get("aos.agent/SessionWorkflow@1") or {}).get("state_expanded") or {}
status = ((state.get("status") or {}).get("$tag")) or None
finished = bool(state.get("finished"))
host_session_id = state.get("host_session_id")
failure = state.get("failure")
output_ref = result.get("blob_ref") or state.get("output_ref") or session.get("last_output_ref")
assistant_text = result.get("blob_value")

if not isinstance(state, dict) or not state:
    raise SystemExit("failed: missing demiurge keyed state")
if not isinstance(session, dict) or not session:
    raise SystemExit("failed: missing session workflow keyed state")
if not isinstance(host_session_id, str) or not host_session_id:
    raise SystemExit(f"failed: host_session_id missing status={status} failure={failure}")
if provider == "mock":
    if status in (None, "Idle", "Bootstrapping"):
        raise SystemExit(f"failed: unexpected demiurge status {status}")
    print(
        f"smoke ok: task_id={task_id} status={status} "
        f"host_session_id={host_session_id} provider=mock"
    )
    print("note: mock provider does not perform a real LLM call")
    raise SystemExit(0)
if failure:
    raise SystemExit(f"failed: demiurge state failure={json.dumps(failure, sort_keys=True)}")
if status in (None, "Idle", "Bootstrapping"):
    raise SystemExit(f"failed: unexpected demiurge status {status}")
if not finished:
    terminal = trace.get("terminal_state")
    raise SystemExit(
        f"failed: task {task_id} did not finish status={status} terminal={terminal}"
    )

if assistant_text is None and output_ref:
    try:
        blob_doc = json.loads(
            subprocess.check_output(
                [aos_bin, "--json", "--quiet", "cas", "get", str(output_ref)],
                stderr=subprocess.DEVNULL,
            ).decode("utf-8", errors="replace")
        )
        data_b64 = ((blob_doc.get("data") or {}).get("data_b64")) or ""
        if data_b64:
            assistant_text = json.loads(base64.b64decode(data_b64).decode("utf-8", errors="replace")).get("assistant_text")
    except Exception:
        assistant_text = None

print(f"smoke ok: task_id={task_id} status={status} host_session_id={host_session_id}")
if assistant_text:
    print("")
    print("assistant_response:")
    print(assistant_text)
PY

echo
echo "Demiurge local smoke passed"
echo "Docs: http://127.0.0.1:9080/docs/"
