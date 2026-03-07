#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORLD_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_DIR="$(cd "${WORLD_DIR}/../.." && pwd)"

TASK_ID="33333333-3333-3333-3333-333333333333"
AOS_TIMEOUT_MS="${AOS_TIMEOUT_MS:-180000}"

# Rebuild CLI every smoke run so local adapter/workflow changes are picked up.
(cd "${REPO_DIR}" && cargo build -p aos-cli >/dev/null)
AOS_BIN="${REPO_DIR}/target/debug/aos"

# Ensure host/system workflow modules can be resolved during `aos push`.
(
  cd "${REPO_DIR}"
  cargo build -p aos-sys --target wasm32-unknown-unknown >/dev/null
  cargo build -p aos-agent --bin session_workflow --target wasm32-unknown-unknown >/dev/null
)

SESSION_WASM="${REPO_DIR}/target/wasm32-unknown-unknown/debug/session_workflow.wasm"
SESSION_HASH="$(shasum -a 256 "${SESSION_WASM}" | awk '{print $1}')"
SESSION_MODULE_DIR="${WORLD_DIR}/modules/aos.agent"
SESSION_MODULE_PATH="${SESSION_MODULE_DIR}/SessionWorkflow@1-${SESSION_HASH}.wasm"
mkdir -p "${SESSION_MODULE_DIR}"
rm -f "${SESSION_MODULE_DIR}"/SessionWorkflow@1-*.wasm
cp "${SESSION_WASM}" "${SESSION_MODULE_PATH}"

rm -rf "${WORLD_DIR}/.aos"
"${AOS_BIN}" -w "${WORLD_DIR}" init
"${AOS_BIN}" -w "${WORLD_DIR}" push

TASK_EVENT="$(python3 - <<'PY' "${TASK_ID}" "${REPO_DIR}"
import json
import sys
print(json.dumps({
  "task_id": sys.argv[1],
  "observed_at_ns": 1,
  "workdir": sys.argv[2],
  "task": "Read README.md and summarize the project name in one sentence.",
  "config": {
    "provider": "mock",
    "model": "gpt-mock",
    "max_tokens": 128,
    "tool_profile": "openai",
    "allowed_tools": ["host.fs.read_file"],
    "tool_enable": ["host.fs.read_file"],
    "tool_disable": None,
    "tool_force": None,
    "reasoning_effort": None,
    "session_ttl_ns": None,
  }
}, separators=(",", ":")))
PY
)"

"${AOS_BIN}" --timeout-ms "${AOS_TIMEOUT_MS}" -w "${WORLD_DIR}" event send "demiurge/TaskSubmitted@1" "${TASK_EVENT}"

# One noop ingress advances the session workflow bootstrap in batch mode
# (tool registry + host session update + first run request).
NOOP_INGRESS="$(python3 - <<'PY' "${TASK_ID}"
import json
import sys
print(json.dumps({
  "session_id": sys.argv[1],
  "observed_at_ns": 2,
  "ingress": {"$tag": "Noop"}
}, separators=(",", ":")))
PY
)"
"${AOS_BIN}" --timeout-ms "${AOS_TIMEOUT_MS}" -w "${WORLD_DIR}" event send "aos.agent/SessionIngress@1" "${NOOP_INGRESS}" >/dev/null

# Verify both cells exist and Demiurge completed bootstrap.
STATE_JSON="$("${AOS_BIN}" --json --quiet -w "${WORLD_DIR}" state get demiurge/Demiurge@1 --key "${TASK_ID}")"
SESSION_STATE_JSON="$("${AOS_BIN}" --json --quiet -w "${WORLD_DIR}" state get aos.agent/SessionWorkflow@1 --key "${TASK_ID}")"

python3 - <<'PY' "${STATE_JSON}" "${SESSION_STATE_JSON}"
import json
import sys

state_raw = sys.argv[1]
session_raw = sys.argv[2]

state = json.loads(state_raw)
session = json.loads(session_raw)

state_data = state.get("data")
session_data = session.get("data")

if not isinstance(state_data, dict):
    raise SystemExit("failed: missing demiurge keyed state")
if not isinstance(session_data, dict):
    raise SystemExit("failed: missing session workflow keyed state")

status = (state_data.get("status") or {}).get("$tag")
input_ref = state_data.get("input_ref")
host_session_id = state_data.get("host_session_id")
failure = state_data.get("failure")

if not isinstance(input_ref, str) or not input_ref.startswith("sha256:"):
    raise SystemExit("failed: demiurge input_ref missing")
if not isinstance(host_session_id, str) or not host_session_id:
    raise SystemExit(f"failed: host_session_id missing status={status} failure={failure}")
if status in (None, "Idle", "Bootstrapping"):
    raise SystemExit(f"failed: unexpected status {status} failure={failure}")

print(f"smoke ok: status={status} host_session_id={host_session_id}")
PY

echo "Demiurge task-submit smoke passed"
