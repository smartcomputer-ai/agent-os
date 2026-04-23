#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORLD_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_DIR="$(cd "${WORLD_DIR}/../.." && pwd)"

AOS_TIMEOUT_MS="${AOS_TIMEOUT_MS:-180000}"
AOS_POLL_INTERVAL_SEC="${AOS_POLL_INTERVAL_SEC:-1}"
AOS_DEMIURGE_BIND="${AOS_DEMIURGE_BIND:-127.0.0.1:9011}"

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
  --provider <id>         LLM provider id.
  --model <id>            Model id.
  --max-tokens <n>        Max completion tokens (default: 256).
  --tool-profile <id>     Tool profile (default: openai).
  --allowed-tools <csv>   Allowed tools CSV (default: host.fs.read_file).
  --tool-enable <csv>     Enabled tools CSV (default: host.fs.read_file).
  -h, --help              Show help.

Notes:
  - Starts an isolated local `aos node` with a temporary CLI config.
  - Creates a fresh Demiurge world, syncs secrets from worlds/demiurge/.env,
    submits one task, runs the patch/noop path, then shuts the node down.
  - Provider selection defaults to `openai-responses` when `OPENAI_API_KEY` is
    present and `anthropic` when `ANTHROPIC_API_KEY` is present.
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
    echo "missing provider: set OPENAI_API_KEY, ANTHROPIC_API_KEY, or pass --provider" >&2
    exit 2
  fi
fi

if [[ "${PROVIDER}" == "mock" ]]; then
  echo "provider 'mock' is not supported by the current end-to-end node smoke path" >&2
  exit 2
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

echo "building local demiurge prerequisites"
(
  cd "${REPO_DIR}"
  cargo build -p aos-cli >/dev/null
  cargo build -p aos-agent --bin session_workflow --target wasm32-unknown-unknown >/dev/null
)

AOS_BIN="${REPO_DIR}/target/debug/aos"
RUN_DIR="$(mktemp -d "${REPO_DIR}/target/demiurge-smoke.XXXXXX")"
PROFILE="demiurge-smoke-$$"
CONFIG_PATH="${RUN_DIR}/aos-config.json"

cleanup() {
  AOS_CONFIG="${CONFIG_PATH}" "${AOS_BIN}" node down \
    --root "${RUN_DIR}/node" \
    --profile "${PROFILE}" \
    --force >/dev/null 2>&1 || true
  if [[ "${AOS_DEMIURGE_KEEP_SMOKE_ROOT:-}" != "1" ]]; then
    rm -rf "${RUN_DIR}"
  else
    echo "kept smoke root: ${RUN_DIR}"
  fi
}
trap cleanup EXIT

echo "starting isolated node on ${AOS_DEMIURGE_BIND}"
AOS_CONFIG="${CONFIG_PATH}" "${AOS_BIN}" node up \
  --root "${RUN_DIR}/node" \
  --bind "${AOS_DEMIURGE_BIND}" \
  --profile "${PROFILE}" \
  --select \
  --background >/dev/null

echo "creating Demiurge world"
AOS_CONFIG="${CONFIG_PATH}" "${AOS_BIN}" world create \
  --local-root "${WORLD_DIR}" \
  --sync-secrets \
  --select >/dev/null

echo "submitting Demiurge task ${TASK_ID}"
AOS_CONFIG="${CONFIG_PATH}" \
AOS_TIMEOUT_MS="${AOS_TIMEOUT_MS}" \
POLL_INTERVAL_SEC="${AOS_POLL_INTERVAL_SEC}" \
"${SCRIPT_DIR}/demiurge_task.sh" \
  --task-id "${TASK_ID}" \
  --task "${TASK_TEXT}" \
  --workdir "${WORKDIR_VALUE}" \
  --provider "${PROVIDER}" \
  --model "${MODEL}" \
  --max-tokens "${MAX_TOKENS}" \
  --tool-profile "${TOOL_PROFILE}" \
  --allowed-tools "${ALLOWED_TOOLS_CSV}" \
  --tool-enable "${TOOL_ENABLE_CSV}"

echo "checking patch/noop path"
AOS_CONFIG="${CONFIG_PATH}" "${AOS_BIN}" world patch \
  --local-root "${WORLD_DIR}" \
  --sync-secrets >/dev/null

echo
echo "Demiurge local smoke passed"
