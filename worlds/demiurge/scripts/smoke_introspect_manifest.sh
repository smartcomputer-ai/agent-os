#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORLD_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_DIR="$(cd "${WORLD_DIR}/../.." && pwd)"

export AOS_WORLD="${WORLD_DIR}"
export AOS_TIMEOUT_MS="${AOS_TIMEOUT_MS:-180000}"
unset AOS_STORE AOS_AIR AOS_WORKFLOW AOS_CONTROL

SESSION_ID="22222222-2222-2222-2222-222222222222"
OBSERVED_AT=1
INPUT_REF="sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

DEBUG_ARTIFACT_DIR=""

emit_debug_artifacts() {
  if [ -n "${DEBUG_ARTIFACT_DIR}" ]; then
    return
  fi
  DEBUG_ARTIFACT_DIR="${WORLD_DIR}/.aos/debug/smoke-$(date +%Y%m%d-%H%M%S)"
  mkdir -p "${DEBUG_ARTIFACT_DIR}"
  "${AOS_BIN}" --json --quiet -w "${WORLD_DIR}" journal tail --limit 500 \
    >"${DEBUG_ARTIFACT_DIR}/journal-tail.json" 2>/dev/null || true
  echo "Debug artifacts: ${DEBUG_ARTIFACT_DIR}" >&2
}

fail() {
  local msg="${1:-smoke failed}"
  echo "${msg}" >&2
  emit_debug_artifacts
  exit 1
}

run_json_file() {
  local out
  out="$(mktemp -t demiurge-json-XXXX)"
  "$@" >"${out}"
  if [ ! -s "${out}" ]; then
    echo "Command produced no JSON output: $*" >&2
    rm -f "${out}"
    exit 1
  fi
  echo "${out}"
}

next_observed_at() {
  local current="${OBSERVED_AT}"
  OBSERVED_AT=$((OBSERVED_AT + 1))
  echo "${current}"
}

ingress_payload() {
  local ingress_json="$1"
  local observed_at
  observed_at="$(next_observed_at)"
  python3 - <<'PY' "${SESSION_ID}" "${observed_at}" "${ingress_json}"
import json, sys
session_id = sys.argv[1]
observed_at = int(sys.argv[2])
ingress = json.loads(sys.argv[3])
payload = {
    "session_id": session_id,
    "observed_at_ns": observed_at,
    "ingress": ingress,
}
print(json.dumps(payload, separators=(",", ":")))
PY
}

send_session_ingress() {
  local ingress_kind_json="$1"
  local payload
  payload="$(ingress_payload "${ingress_kind_json}")"
  "${AOS_BIN}" -w "${WORLD_DIR}" event send "aos.agent/SessionIngress@1" "${payload}"
}

read_state_json() {
  "${AOS_BIN}" --json --quiet -w "${WORLD_DIR}" state get demiurge/Demiurge@1 --key "${SESSION_ID}"
}

rm -rf "${WORLD_DIR}/.aos"

pushd "${REPO_DIR}" >/dev/null
cargo build -p aos-sys --target wasm32-unknown-unknown >/dev/null
cargo run -p aos-cli -- init -w "${WORLD_DIR}"
cargo run -p aos-cli -- push -w "${WORLD_DIR}"
if [ ! -x "${REPO_DIR}/target/debug/aos" ]; then
  cargo build -p aos-cli
fi
AOS_BIN="${REPO_DIR}/target/debug/aos"

export AOS_MODE=batch

# Direct refs run.
PROMPT_FILE="$(run_json_file "${AOS_BIN}" --json --quiet -w "${WORLD_DIR}" blob put "@${WORLD_DIR}/agent-ws/prompts/packs/default.json")"
PROMPT_HASH="$(python3 - <<'PY' "${PROMPT_FILE}"
import json,sys
print(json.load(open(sys.argv[1], 'r', encoding='utf-8'))['data']['hash'])
PY
)"
rm -f "${PROMPT_FILE}"

DIRECT_RUN_INGRESS="$(python3 - <<'PY' "${INPUT_REF}" "${PROMPT_HASH}"
import json,sys
input_ref,prompt_hash=sys.argv[1],sys.argv[2]
print(json.dumps({
  '$tag':'RunRequested',
  '$value':{
    'input_ref': input_ref,
    'run_overrides': {
      'provider':'mock',
      'model':'gpt-mock',
      'reasoning_effort': None,
      'max_tokens': 128,
      'default_prompt_refs': [prompt_hash],
      'default_tool_profile': 'openai',
      'default_tool_enable': ['host.session.open'],
      'default_tool_disable': None,
      'default_tool_force': None,
    }
  }
}, separators=(',',':')))
PY
)"
send_session_ingress "${DIRECT_RUN_INGRESS}"

STATE_JSON="$(read_state_json)"
DIRECT_CHECK="$(python3 - <<'PY' "${STATE_JSON}"
import json,sys
raw=sys.argv[1]
start=raw.find('{')
if start == -1:
    print('no')
    raise SystemExit
obj=json.loads(raw[start:])
session=((obj.get('data') or {}).get('session') or {})
cfg=session.get('active_run_config') or {}
prompt_refs=cfg.get('prompt_refs') or []
tool_profile=cfg.get('tool_profile')
strict=(len(prompt_refs)==1 and prompt_refs[0].startswith('sha256:') and tool_profile in {'openai','anthropic','gemini'})
lifecycle=((session.get('lifecycle') or {}).get('$tag'))
next_run_seq=session.get('next_run_seq') or 0
weak=(next_run_seq >= 1 and lifecycle in {'Running','WaitingInput','Failed'})
ok=(strict or weak)
print('yes' if ok else 'no')
PY
)"
if [ "${DIRECT_CHECK}" != "yes" ]; then
  echo "${STATE_JSON}" >&2
  fail "direct prompt/tool refs run did not advance session as expected"
fi

echo "Demiurge workflow-native smoke passed"
popd >/dev/null
