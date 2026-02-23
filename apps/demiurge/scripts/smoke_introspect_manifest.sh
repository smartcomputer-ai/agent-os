#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORLD_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_DIR="$(cd "${WORLD_DIR}/../.." && pwd)"

export AOS_WORLD="${WORLD_DIR}"
export AOS_TIMEOUT_MS="${AOS_TIMEOUT_MS:-180000}"
unset AOS_STORE AOS_AIR AOS_REDUCER AOS_CONTROL

SESSION_ID="22222222-2222-2222-2222-222222222222"
EVENT_STEP=1
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

next_step_epoch() {
  local current="${EVENT_STEP}"
  EVENT_STEP=$((EVENT_STEP + 1))
  echo "${current}"
}

session_event_payload() {
  local event_json="$1"
  local step_epoch
  step_epoch="$(next_step_epoch)"
  python3 - <<'PY' "${SESSION_ID}" "${step_epoch}" "${event_json}"
import json, sys
session_id = sys.argv[1]
step_epoch = int(sys.argv[2])
event_kind = json.loads(sys.argv[3])
payload = {
    "session_id": session_id,
    "run_id": None,
    "turn_id": None,
    "step_id": None,
    "session_epoch": 0,
    "step_epoch": step_epoch,
    "event": event_kind,
}
print(json.dumps(payload, separators=(",", ":")))
PY
}

send_session_event() {
  local event_kind_json="$1"
  local payload
  payload="$(session_event_payload "${event_kind_json}")"
  "${AOS_BIN}" -w "${WORLD_DIR}" event send "aos.agent/SessionEvent@1" "${payload}"
}

read_state_json() {
  "${AOS_BIN}" --json --quiet -w "${WORLD_DIR}" state get demiurge/Demiurge@1
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

# 1) Workspace sync + apply.
send_session_event '{"$tag":"WorkspaceSyncRequested","$value":{"workspace_binding":{"workspace":"demiurge","version":null},"prompt_pack":"default","tool_catalog":"default","known_version":null}}'
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
pending=((obj.get('data') or {}).get('pending_workspace_snapshot'))
print('yes' if isinstance(pending, dict) else 'no')
PY
  )"
  if [ "${HAS_PENDING}" = "yes" ]; then
    break
  fi
done
if [ "${HAS_PENDING}" != "yes" ]; then
  echo "${STATE_JSON}" >&2
  fail "workspace sync did not stage pending snapshot"
fi

send_session_event '{"$tag":"WorkspaceApplyRequested","$value":{"mode":{"$tag":"NextRun"}}}'
send_session_event '{"$tag":"RunRequested","$value":{"input_ref":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","run_overrides":{"provider":"mock","model":"gpt-mock","reasoning_effort":null,"max_tokens":256,"workspace_binding":{"workspace":"demiurge","version":null},"default_prompt_pack":"default","default_prompt_refs":null,"default_tool_catalog":"default","default_tool_refs":null}}}'
send_session_event '{"$tag":"RunStarted"}'

STATE_JSON="$(read_state_json)"
STATE_TMP="$(mktemp -t demiurge-state-XXXX.json)"
printf '%s' "${STATE_JSON}" > "${STATE_TMP}"

TOOL_BATCH_JSON="$(python3 - <<'PY' "${STATE_TMP}" "${SESSION_ID}" "$(next_step_epoch)"
import json,sys
state_path,session_id,step_epoch=sys.argv[1],sys.argv[2],int(sys.argv[3])
obj=json.loads(open(state_path,'r',encoding='utf-8').read())
state=obj.get('data') or {}
step_id=state.get('active_step_id')
if not isinstance(step_id, dict):
    raise SystemExit('missing active_step_id')
payload={
  'session_id': session_id,
  'run_id': None,
  'turn_id': None,
  'step_id': None,
  'session_epoch': 0,
  'step_epoch': step_epoch,
  'event': {
    '$tag':'ToolBatchStarted',
    '$value': {
      'tool_batch_id': {'step_id': step_id, 'batch_seq': 1},
      'expected_call_ids': ['call_1']
    }
  }
}
print(json.dumps(payload,separators=(',',':')))
PY
)"
"${AOS_BIN}" -w "${WORLD_DIR}" event send "aos.agent/SessionEvent@1" "${TOOL_BATCH_JSON}"

TOOL_REQ_JSON="$(python3 - <<'PY' "${STATE_TMP}" "${SESSION_ID}"
import json,sys
state_path,session_id=sys.argv[1],sys.argv[2]
obj=json.loads(open(state_path,'r',encoding='utf-8').read())
state=obj.get('data') or {}
run_id=state.get('active_run_id')
turn_id=state.get('active_turn_id')
step_id=state.get('active_step_id')
step_epoch=state.get('step_epoch')
if not isinstance(step_id, dict):
    raise SystemExit('missing active_step_id')
payload={
  'session_id': session_id,
  'run_id': run_id,
  'turn_id': turn_id,
  'step_id': step_id,
  'session_epoch': 0,
  'step_epoch': step_epoch,
  'tool_batch_id': {'step_id': step_id, 'batch_seq': 1},
  'call_id': 'call_1',
  'finalize_batch': True,
  'params': {
    '$tag': 'IntrospectManifest',
    '$value': {'consistency': 'head'}
  }
}
print(json.dumps(payload,separators=(',',':')))
PY
)"
"${AOS_BIN}" -w "${WORLD_DIR}" event send "demiurge/ToolCallRequested@1" "${TOOL_REQ_JSON}"

for _ in $(seq 1 10); do
  OUTCOME="$("${AOS_BIN}" --json --quiet -w "${WORLD_DIR}" run --batch 2>/dev/null || true)"
  if [ -n "${OUTCOME}" ]; then
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
  else
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
state=obj.get('data') or {}
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

send_session_event '{"$tag":"StepBoundary"}'
send_session_event '{"$tag":"RunCompleted"}'
"${AOS_BIN}" --quiet -w "${WORLD_DIR}" run --batch >/dev/null

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

DIRECT_RUN_EVENT="$(python3 - <<'PY' "${INPUT_REF}" "${PROMPT_HASH}" "${TOOL_HASH}"
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
send_session_event "${DIRECT_RUN_EVENT}"
send_session_event '{"$tag":"RunStarted"}'

STATE_JSON="$(read_state_json)"
DIRECT_CHECK="$(python3 - <<'PY' "${STATE_JSON}"
import json,sys
raw=sys.argv[1]
start=raw.find('{')
if start == -1:
    print('no')
    raise SystemExit
obj=json.loads(raw[start:])
cfg=((obj.get('data') or {}).get('active_run_config') or {})
prompt_refs=cfg.get('prompt_refs') or []
tool_refs=cfg.get('tool_refs') or []
workspace_binding=cfg.get('workspace_binding')
ok=(len(prompt_refs)==1 and prompt_refs[0].startswith('sha256:') and len(tool_refs)==1 and tool_refs[0].startswith('sha256:') and workspace_binding is None)
print('yes' if ok else 'no')
PY
)"
if [ "${DIRECT_CHECK}" != "yes" ]; then
  echo "${STATE_JSON}" >&2
  fail "direct prompt/tool refs run config was not applied"
fi

send_session_event '{"$tag":"RunCompleted"}'
"${AOS_BIN}" --quiet -w "${WORLD_DIR}" run --batch >/dev/null

FINAL_STATE="$(read_state_json)"
FINAL_LIFECYCLE="$(python3 - <<'PY' "${FINAL_STATE}"
import json,sys
raw=sys.argv[1]
start=raw.find('{')
if start == -1:
    print('')
    raise SystemExit
obj=json.loads(raw[start:])
print(((obj.get('data') or {}).get('lifecycle') or {}).get('$tag', ''))
PY
)"
if [ "${FINAL_LIFECYCLE}" != "Completed" ]; then
  echo "${FINAL_STATE}" >&2
  fail "expected final lifecycle Completed"
fi

rm -f "${STATE_TMP}"

echo "Demiurge SDK smoke passed"
popd >/dev/null
