#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${REPO_ROOT}"

AOS_BIN="${AOS_BIN:-target/debug/aos}"
STATE_ROOT="${STATE_ROOT:-${REPO_ROOT}/.aos-node}"
LOG_FILE="${LOG_FILE:-${STATE_ROOT}/repro-node.log}"
TASK_TEXT="${TASK_TEXT:-Echo YO-YOO.}"
STARTUP_WAIT_SECS="${STARTUP_WAIT_SECS:-5}"

cleanup() {
  "${AOS_BIN}" node down --json >/dev/null 2>&1 || true
}
trap cleanup EXIT

start_node() {
  mkdir -p "${STATE_ROOT}"
  : > "${LOG_FILE}"
  "${AOS_BIN}" node up --root "${REPO_ROOT}" --journal-backend kafka \
    --blob-backend object-store --background \
    >"${LOG_FILE}" 2>&1
  sleep "${STARTUP_WAIT_SECS}"
  curl -sf http://127.0.0.1:9010/v1/health >/dev/null
}

stop_node() {
  "${AOS_BIN}" node down --json >/dev/null 2>&1 || true
}

echo "Resetting node environment..."
"${AOS_BIN}" node down --json >/dev/null 2>&1 || true
./dev/hosted/hosted-topics-reset.sh >/dev/null
rm -rf "${STATE_ROOT}"
mkdir -p "${STATE_ROOT}"

echo "Starting node for world creation..."
start_node
"${AOS_BIN}" node use >/dev/null
"${AOS_BIN}" world create --local-root worlds/demiurge --sync-secrets --select >/dev/null

echo "Restarting node before task traffic..."
stop_node
start_node

echo "Running task 1..."
worlds/demiurge/scripts/demiurge_task.sh --task "${TASK_TEXT}" >/dev/null

echo "Running task 2..."
worlds/demiurge/scripts/demiurge_task.sh --task "${TASK_TEXT}" >/dev/null

echo "Restarting node after task traffic..."
stop_node
start_node

if grep -q "disabling world after activation error" "${LOG_FILE}"; then
  echo "Reproduced corruption bug."
  grep "disabling world after activation error" "${LOG_FILE}"
  exit 0
fi

echo "Did not reproduce corruption bug."
echo "Last node log lines:"
tail -n 40 "${LOG_FILE}" || true
exit 1
