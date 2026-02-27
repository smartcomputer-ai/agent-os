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

send_tool_request() {
  local params_json="$1"
  local tool_batch_json="$2"
  local finalize="${3:-true}"
  local observed_at
  observed_at="$(next_observed_at)"
  local payload
  payload="$(python3 - <<'PY' "${SESSION_ID}" "${observed_at}" "${tool_batch_json}" "${finalize}" "${params_json}"
import json,sys
session_id = sys.argv[1]
observed_at = int(sys.argv[2])
tool_batch_id = json.loads(sys.argv[3])
finalize_batch = sys.argv[4].lower() == "true"
params = json.loads(sys.argv[5])
payload = {
    "session_id": session_id,
    "observed_at_ns": observed_at,
    "tool_batch_id": tool_batch_id,
    "call_id": "call_1",
    "finalize_batch": finalize_batch,
    "params": params,
}
print(json.dumps(payload, separators=(",", ":")))
PY
)"
  "${AOS_BIN}" -w "${WORLD_DIR}" event send "demiurge/ToolCallRequested@1" "${payload}"
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

# 1) Workspace snapshot apply + run.
send_session_ingress '{"$tag":"WorkspaceSyncRequested","$value":{"workspace_binding":{"workspace":"demiurge","version":null},"prompt_pack":"default","tool_catalog":"default"}}'
send_session_ingress '{"$tag":"WorkspaceSnapshotReady","$value":{"snapshot":{"workspace":"demiurge","version":null,"root_hash":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","index_ref":null,"prompt_pack":"default","tool_catalog":"default","prompt_pack_ref":null,"tool_catalog_ref":null},"prompt_pack_bytes":null,"tool_catalog_bytes":null}}'

HAS_PENDING="no"
for _ in $(seq 1 10); do
  "${AOS_BIN}" --quiet -w "${WORLD_DIR}" run --batch >/dev/null || true
  STATE_JSON="$(read_state_json)"
  HAS_PENDING="$(python3 - <<'PY' "${STATE_JSON}"
import json,sys
raw=sys.argv[1]
start=raw.find('{')
if start == -1:
    print('no')
    raise SystemExit
obj=json.loads(raw[start:])
pending=(((obj.get('data') or {}).get('session') or {}).get('pending_workspace_snapshot'))
print('yes' if isinstance(pending, dict) else 'no')
PY
  )"
  if [ "${HAS_PENDING}" = "yes" ]; then
    break
  fi
done
if [ "${HAS_PENDING}" != "yes" ]; then
  echo "${STATE_JSON}" >&2
  fail "workspace snapshot did not stage pending snapshot"
fi

send_session_ingress '{"$tag":"WorkspaceApplyRequested","$value":{"mode":{"$tag":"ImmediateIfIdle"}}}'

TOOL_BATCH_JSON="$(python3 - <<'PY' "${SESSION_ID}"
import json,sys
session_id=sys.argv[1]
print(json.dumps({
  'run_id': {
    'session_id': session_id,
    'run_seq': 1
  },
  'batch_seq': 1
}, separators=(',',':')))
PY
)"

TOOL_BATCH_INGRESS="$(python3 - <<'PY' "${TOOL_BATCH_JSON}"
import json,sys
tool_batch_id=json.loads(sys.argv[1])
print(json.dumps({
  '$tag':'ToolBatchStarted',
  '$value':{
    'tool_batch_id': tool_batch_id,
    'intent_id': 'sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
    'params_hash': None,
    'expected_call_ids': ['call_1']
  }
}, separators=(',',':')))
PY
)"
send_session_ingress "${TOOL_BATCH_INGRESS}"

send_tool_request '{"$tag":"WorkspaceReadBytes","$value":{"workspace":"demiurge","version":null,"path":"agent.workspace.json"}}' "${TOOL_BATCH_JSON}" "true"

for _ in $(seq 1 20); do
  OUTCOME="$("${AOS_BIN}" --json --quiet -w "${WORLD_DIR}" run --batch 2>/dev/null || true)"
  if [ -z "${OUTCOME}" ]; then
    break
  fi
  DISPATCHED="$(python3 - <<'PY' "${OUTCOME}"
import json,sys
raw=sys.argv[1].strip()
if not raw:
    print('0,0')
    raise SystemExit
start=raw.find('{')
obj=json.loads(raw[start:])
data=obj.get('data') or {}
print(f"{data.get('effects_dispatched',0)},{data.get('receipts_applied',0)}")
PY
  )"
  if [ "${DISPATCHED}" = "0,0" ]; then
    break
  fi
done

STATE_JSON="$(read_state_json)"
CHECK_RESULT="$(python3 - <<'PY' "${STATE_JSON}"
import json,sys
raw=sys.argv[1]
start=raw.find('{')
if start == -1:
    print('no||')
    raise SystemExit
obj=json.loads(raw[start:])
state=(obj.get('data') or {}).get('session') or {}
batch=state.get('active_tool_batch') or {}
status=((batch.get('call_status') or {}).get('call_1') or {}).get('$tag')
prompt_pack=((state.get('active_workspace_snapshot') or {}).get('prompt_pack'))
tool_catalog=((state.get('active_workspace_snapshot') or {}).get('tool_catalog'))
ok='yes' if status == 'Succeeded' else 'no'
print(f"{ok}|{prompt_pack or ''}|{tool_catalog or ''}")
PY
)"
OK_FLAG="${CHECK_RESULT%%|*}"
REMAINDER="${CHECK_RESULT#*|}"
ACTIVE_PROMPT="${REMAINDER%%|*}"
ACTIVE_TOOL="${REMAINDER#*|}"

if [ "${OK_FLAG}" != "yes" ]; then
  echo "${STATE_JSON}" >&2
  fail "tool batch was not settled with a successful call"
fi
if [ "${ACTIVE_PROMPT}" != "default" ] || [ "${ACTIVE_TOOL}" != "default" ]; then
  echo "${STATE_JSON}" >&2
  fail "workspace snapshot defaults were not applied to active run"
fi

# 2) Direct refs run (no workspace binding required).
PROMPT_FILE="$(run_json_file "${AOS_BIN}" --json --quiet -w "${WORLD_DIR}" blob put "@${WORLD_DIR}/agent-ws/prompts/packs/default.json")"
PROMPT_HASH="$(python3 - <<'PY' "${PROMPT_FILE}"
import json,sys
print(json.load(open(sys.argv[1], 'r', encoding='utf-8'))['data']['hash'])
PY
)"
rm -f "${PROMPT_FILE}"

TOOL_FILE="$(run_json_file "${AOS_BIN}" --json --quiet -w "${WORLD_DIR}" blob put "@${WORLD_DIR}/agent-ws/tools/catalogs/default.json")"
TOOL_HASH="$(python3 - <<'PY' "${TOOL_FILE}"
import json,sys
print(json.load(open(sys.argv[1], 'r', encoding='utf-8'))['data']['hash'])
PY
)"
rm -f "${TOOL_FILE}"

DIRECT_RUN_INGRESS="$(python3 - <<'PY' "${INPUT_REF}" "${PROMPT_HASH}" "${TOOL_HASH}"
import json,sys
input_ref,prompt_hash,tool_hash=sys.argv[1],sys.argv[2],sys.argv[3]
print(json.dumps({
  '$tag':'RunRequested',
  '$value':{
    'input_ref': input_ref,
    'run_overrides': {
      'provider':'mock',
      'model':'gpt-mock',
      'reasoning_effort': None,
      'max_tokens': 128,
      'workspace_binding': None,
      'default_prompt_pack': None,
      'default_prompt_refs': [prompt_hash],
      'default_tool_catalog': None,
      'default_tool_refs': [tool_hash],
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
tool_refs=cfg.get('tool_refs') or []
workspace_binding=cfg.get('workspace_binding')
strict=(len(prompt_refs)==1 and prompt_refs[0].startswith('sha256:') and len(tool_refs)==1 and tool_refs[0].startswith('sha256:') and workspace_binding is None)
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
