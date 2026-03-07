#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORLD_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_DIR="$(cd "${WORLD_DIR}/../.." && pwd)"

AOS_TIMEOUT_MS="${AOS_TIMEOUT_MS:-180000}"
RUN_DAEMON=0
RESET_STORE=0
SKIP_BUILD=0

usage() {
  cat <<'USAGE'
Usage:
  worlds/demiurge/scripts/demiurge_up.sh [options]

Options:
  --run         Start daemon after init+push (foreground).
  --reset       Remove worlds/demiurge/.aos before init+push.
  --no-build    Skip cargo builds.
  -h, --help    Show this help.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --run)
      RUN_DAEMON=1
      shift
      ;;
    --reset)
      RESET_STORE=1
      shift
      ;;
    --no-build)
      SKIP_BUILD=1
      shift
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

if [[ "${SKIP_BUILD}" -eq 0 ]]; then
  (
    cd "${REPO_DIR}"
    cargo build -p aos-cli >/dev/null
    cargo build -p aos-sys --target wasm32-unknown-unknown >/dev/null
    cargo build -p aos-agent --bin session_workflow --target wasm32-unknown-unknown >/dev/null
  )
fi

AOS_BIN="${REPO_DIR}/target/debug/aos"
if [[ ! -x "${AOS_BIN}" ]]; then
  AOS_BIN="aos"
fi

SESSION_WASM="${REPO_DIR}/target/wasm32-unknown-unknown/debug/session_workflow.wasm"
if [[ ! -f "${SESSION_WASM}" ]]; then
  echo "missing wasm at ${SESSION_WASM}; run without --no-build first" >&2
  exit 1
fi

SESSION_HASH="$(shasum -a 256 "${SESSION_WASM}" | awk '{print $1}')"
SESSION_MODULE_DIR="${WORLD_DIR}/modules/aos.agent"
SESSION_MODULE_PATH="${SESSION_MODULE_DIR}/SessionWorkflow@1-${SESSION_HASH}.wasm"
mkdir -p "${SESSION_MODULE_DIR}"
rm -f "${SESSION_MODULE_DIR}"/SessionWorkflow@1-*.wasm
cp "${SESSION_WASM}" "${SESSION_MODULE_PATH}"

if [[ "${RESET_STORE}" -eq 1 ]]; then
  rm -rf "${WORLD_DIR}/.aos"
fi

if [[ ! -d "${WORLD_DIR}/.aos" ]]; then
  "${AOS_BIN}" -w "${WORLD_DIR}" init
fi
"${AOS_BIN}" -w "${WORLD_DIR}" push

echo "world ready: ${WORLD_DIR}"
echo "aos bin: ${AOS_BIN}"
echo "session workflow wasm: ${SESSION_MODULE_PATH}"

if [[ "${RUN_DAEMON}" -eq 1 ]]; then
  exec "${AOS_BIN}" --timeout-ms "${AOS_TIMEOUT_MS}" -w "${WORLD_DIR}" run
fi

