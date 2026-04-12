#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${REPO_ROOT}"

NODE_BIN="${NODE_BIN:-target/debug/aos-node-hosted}"
AOS_BIN="${AOS_BIN:-target/debug/aos}"
STATE_ROOT="${STATE_ROOT:-${REPO_ROOT}/.aos-hosted}"
LOG_FILE="${LOG_FILE:-${STATE_ROOT}/repro-node.log}"
TASK_TEXT="${TASK_TEXT:-Echo YO-YOO.}"
STARTUP_WAIT_SECS="${STARTUP_WAIT_SECS:-5}"

cleanup() {
  if [[ -n "${NODE_PID:-}" ]] && kill -0 "${NODE_PID}" >/dev/null 2>&1; then
    kill "${NODE_PID}" >/dev/null 2>&1 || true
    wait "${NODE_PID}" 2>/dev/null || true
  fi
}
trap cleanup EXIT

start_node() {
  mkdir -p "${STATE_ROOT}"
  : > "${LOG_FILE}"
  "${NODE_BIN}" >"${LOG_FILE}" 2>&1 &
  NODE_PID=$!
  sleep "${STARTUP_WAIT_SECS}"
  curl -sf http://127.0.0.1:9011/v1/health >/dev/null
}

stop_node() {
  if [[ -n "${NODE_PID:-}" ]] && kill -0 "${NODE_PID}" >/dev/null 2>&1; then
    kill "${NODE_PID}" >/dev/null 2>&1 || true
    wait "${NODE_PID}" 2>/dev/null || true
  fi
  NODE_PID=""
}

echo "Resetting hosted environment..."
"${AOS_BIN}" hosted down --json >/dev/null 2>&1 || true
./dev/scripts/hosted-topics-reset.sh >/dev/null
rm -rf "${STATE_ROOT}"
mkdir -p "${STATE_ROOT}"

echo "Starting node for world creation..."
start_node
"${AOS_BIN}" hosted use >/dev/null
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

if grep -q "disabling hosted world after activation error" "${LOG_FILE}"; then
  echo "Reproduced corruption bug."
  grep "disabling hosted world after activation error" "${LOG_FILE}"
  exit 0
fi

echo "Did not reproduce corruption bug."
echo "Last node log lines:"
tail -n 40 "${LOG_FILE}" || true
exit 1
