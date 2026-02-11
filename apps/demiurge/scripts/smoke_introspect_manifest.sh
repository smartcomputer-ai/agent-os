#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORLD_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_DIR="$(cd "${WORLD_DIR}/../.." && pwd)"

export AOS_WORLD="${WORLD_DIR}"
export AOS_TIMEOUT_MS="${AOS_TIMEOUT_MS:-180000}"
# Ensure this smoke run is isolated to the demiurge world layout.
unset AOS_STORE AOS_AIR AOS_REDUCER AOS_CONTROL

if [ -f "${WORLD_DIR}/.env" ]; then
  set -a
  . "${WORLD_DIR}/.env"
  set +a
fi

if [ -z "${LLM_API_KEY:-}" ]; then
  echo "LLM_API_KEY is required (set in ${WORLD_DIR}/.env or env)." >&2
  exit 1
fi

DEBUG_ARTIFACT_DIR=""

emit_debug_artifacts() {
  if [ -n "${DEBUG_ARTIFACT_DIR}" ]; then
    return
  fi
  DEBUG_ARTIFACT_DIR="${WORLD_DIR}/.aos/debug/smoke-$(date +%Y%m%d-%H%M%S)"
  mkdir -p "${DEBUG_ARTIFACT_DIR}"

  "${AOS_BIN}" --json --quiet -w "${WORLD_DIR}" journal tail --limit 500 \
    >"${DEBUG_ARTIFACT_DIR}/journal-tail.json" 2>/dev/null || true

  local event_hash
  event_hash="$(
    "${PYTHON_BIN}" - <<'PY' "${DEBUG_ARTIFACT_DIR}/journal-tail.json"
import json,sys
from pathlib import Path

path=Path(sys.argv[1])
if not path.exists():
    sys.exit(0)
try:
    payload=json.loads(path.read_text(encoding="utf-8"))
except Exception:
    sys.exit(0)

entries=((payload.get("data") or {}).get("entries") or [])

def find_str(node, key):
    if isinstance(node, dict):
        v=node.get(key)
        if isinstance(v, str):
            return v
        for child in node.values():
            out=find_str(child, key)
            if out:
                return out
    elif isinstance(node, list):
        for child in node:
            out=find_str(child, key)
            if out:
                return out
    return None

for entry in reversed(entries):
    if (entry.get("kind") or "") != "domain_event":
        continue
    record=entry.get("record")
    schema=find_str(record, "schema") or ""
    event_hash=find_str(record, "event_hash") or ""
    if schema.startswith("demiurge/") and event_hash.startswith("sha256:"):
        print(event_hash)
        sys.exit(0)
sys.exit(0)
PY
  )"

  if [ -n "${event_hash}" ]; then
    "${AOS_BIN}" --json --quiet -w "${WORLD_DIR}" trace --event-hash "${event_hash}" --window-limit 500 \
      >"${DEBUG_ARTIFACT_DIR}/trace.json" 2>/dev/null || true
  fi

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
    cat "${out}" >&2 || true
    rm -f "${out}"
    exit 1
  fi
  echo "${out}"
}

rm -rf "${WORLD_DIR}/.aos"

pushd "${REPO_DIR}" >/dev/null
cargo run -p aos-cli -- init -w "${WORLD_DIR}"
cargo run -p aos-cli -- push -w "${WORLD_DIR}"
if [ ! -x "${REPO_DIR}/target/debug/aos" ]; then
  cargo build -p aos-cli
fi
AOS_BIN="${REPO_DIR}/target/debug/aos"

export AOS_MODE=batch

PYTHON_BIN="${PYTHON_BIN:-python3}"

TOOL_JSON_FILE="$(run_json_file "${AOS_BIN}" --json --quiet -w "${WORLD_DIR}" blob put "@${WORLD_DIR}/tools/introspect.manifest.json")"
TOOL_HASH="$(
  "${PYTHON_BIN}" - <<'PY' "${TOOL_JSON_FILE}"
import json,sys
with open(sys.argv[1], "r", encoding="utf-8") as fh:
    print(json.load(fh)["data"]["hash"])
PY
)"
rm -f "${TOOL_JSON_FILE}"

CHAT_ID="$(
  "${PYTHON_BIN}" - <<'PY'
import time,random,string
suffix="".join(random.choice(string.ascii_lowercase+string.digits) for _ in range(6))
print(f"chat-{int(time.time())}-{suffix}")
PY
)"

MSG_FILE="$(mktemp -t demiurge-msg-XXXX.json)"
cat >"${MSG_FILE}" <<'JSON'
{"role":"user","content":[{"type":"text","text":"Call the introspect_manifest tool now and then reply with a one-sentence summary."}]}
JSON

MSG_JSON_FILE="$(run_json_file "${AOS_BIN}" --json --quiet -w "${WORLD_DIR}" blob put "@${MSG_FILE}")"
MSG_HASH="$(
  "${PYTHON_BIN}" - <<'PY' "${MSG_JSON_FILE}"
import json,sys
with open(sys.argv[1], "r", encoding="utf-8") as fh:
    print(json.load(fh)["data"]["hash"])
PY
)"
rm -f "${MSG_JSON_FILE}"

CHAT_CREATED_JSON="$(printf '{"$tag":"ChatCreated","$value":{"chat_id":"%s","title":"Smoke Test","created_at_ms":1}}' "${CHAT_ID}")"
"${AOS_BIN}" -w "${WORLD_DIR}" event send \
  "demiurge/ChatEvent@1" \
  "${CHAT_CREATED_JSON}"

USER_MESSAGE_JSON="$(printf '{"$tag":"UserMessage","$value":{"chat_id":"%s","request_id":1,"text":"tool smoke","message_ref":"%s","model":"gpt-5.2","provider":"openai-responses","max_tokens":512,"tool_refs":["%s"],"tool_choice":{"$tag":"Required"}}}' "${CHAT_ID}" "${MSG_HASH}" "${TOOL_HASH}")"
"${AOS_BIN}" -w "${WORLD_DIR}" event send \
  "demiurge/ChatEvent@1" \
  "${USER_MESSAGE_JSON}"

"${AOS_BIN}" --quiet -w "${WORLD_DIR}" run --batch >/dev/null

STATE_JSON=""
for _ in $(seq 1 60); do
  "${AOS_BIN}" --quiet -w "${WORLD_DIR}" run --batch >/dev/null
  STATE_JSON="$("${AOS_BIN}" --json --quiet -w "${WORLD_DIR}" state get demiurge/Demiurge@1 --key "${CHAT_ID}" 2>/dev/null || true)"
  STATE_JSON_TRIM="$(printf '%s' "${STATE_JSON}" | tr -d '[:space:]')"
  if [ -z "${STATE_JSON_TRIM}" ]; then
    READY="no"
  else
    READY="$(
      echo "${STATE_JSON}" | "${PYTHON_BIN}" -c '
import json,sys
raw=sys.stdin.read()
start=raw.find("{")
if start == -1:
    print("no")
    sys.exit(0)
data=json.loads(raw[start:]).get("data")
if not isinstance(data, dict):
    print("no")
    sys.exit(0)
messages=data.get("messages", [])
def role_tag(role):
    if isinstance(role, dict):
        return role.get("$tag") or role.get("tag")
    return role
ready=any(
    isinstance(msg, dict)
    and role_tag(msg.get("role")) == "Assistant"
    and msg.get("message_ref")
    for msg in messages
)
print("yes" if ready else "no")
'
)"
  fi
  if [ "${READY}" = "yes" ]; then
    break
  fi
  sleep 1
done

if [ "${READY}" != "yes" ]; then
  echo "${STATE_JSON}" >&2
  fail "Timed out waiting for assistant/tool messages."
fi

ASSISTANT_REFS="$(
  echo "${STATE_JSON}" | "${PYTHON_BIN}" -c '
import json,sys
raw=sys.stdin.read()
start=raw.find("{")
if start == -1:
    sys.exit(1)
data=json.loads(raw[start:])["data"]
messages=data.get("messages", [])
def role_tag(role):
    if isinstance(role, dict):
        return role.get("$tag") or role.get("tag")
    return role
for msg in messages:
    if role_tag(msg.get("role")) == "Assistant":
        ref=msg.get("message_ref")
        if ref:
            print(ref)
'
)"

if [ -z "${ASSISTANT_REFS}" ]; then
  fail "No assistant output_ref found."
fi

TOOL_CALL_REF=""
for ref in ${ASSISTANT_REFS}; do
  if "${AOS_BIN}" -w "${WORLD_DIR}" blob get --raw "${ref}" \
    | "${PYTHON_BIN}" -c '
import json,sys
items=json.load(sys.stdin)
ok=any(
    isinstance(item, dict)
    and item.get("type") == "function_call"
    and item.get("name") == "introspect_manifest"
    for item in items
)
sys.exit(0 if ok else 1)
'
  then
    TOOL_CALL_REF="${ref}"
    break
  fi
done

if [ -z "${TOOL_CALL_REF}" ]; then
  fail "Tool call not found in LLM output."
fi

TOOL_OUTPUT_REF=""
FOLLOWUP_REF=""
for _ in $(seq 1 60); do
  "${AOS_BIN}" --quiet -w "${WORLD_DIR}" run --batch >/dev/null
  STATE_JSON="$("${AOS_BIN}" --json --quiet -w "${WORLD_DIR}" state get demiurge/Demiurge@1 --key "${CHAT_ID}" 2>/dev/null || true)"
  if [ -z "$(printf '%s' "${STATE_JSON}" | tr -d '[:space:]')" ]; then
    sleep 1
    continue
  fi
  ASSISTANT_REFS="$(
    echo "${STATE_JSON}" | "${PYTHON_BIN}" -c '
import json,sys
raw=sys.stdin.read()
start=raw.find("{")
if start == -1:
    sys.exit(1)
data=json.loads(raw[start:])["data"]
messages=data.get("messages", [])
def role_tag(role):
    if isinstance(role, dict):
        return role.get("$tag") or role.get("tag")
    return role
for msg in messages:
    if role_tag(msg.get("role")) == "Assistant":
        ref=msg.get("message_ref")
        if ref:
            print(ref)
'
  )"
  for ref in ${ASSISTANT_REFS}; do
    IS_TOOL_OUTPUT="no"
    if "${AOS_BIN}" -w "${WORLD_DIR}" blob get --raw "${ref}" \
      | "${PYTHON_BIN}" -c '
import json,sys
items=json.load(sys.stdin)
ok=any(
    isinstance(item, dict) and item.get("type") == "function_call_output"
    for item in items
)
sys.exit(0 if ok else 1)
'
    then
      IS_TOOL_OUTPUT="yes"
    fi
    if [ -z "${TOOL_OUTPUT_REF}" ] && [ "${IS_TOOL_OUTPUT}" = "yes" ]; then
      TOOL_OUTPUT_REF="${ref}"
      continue
    fi
    if [ -n "${TOOL_OUTPUT_REF}" ] && [ "${IS_TOOL_OUTPUT}" = "no" ]; then
      if "${AOS_BIN}" -w "${WORLD_DIR}" blob get --raw "${ref}" \
        | "${PYTHON_BIN}" -c '
import json,sys
items=json.load(sys.stdin)
def has_text(items):
    for item in items:
        if not isinstance(item, dict):
            continue
        content=item.get("content")
        if not isinstance(content, list):
            continue
        for part in content:
            if not isinstance(part, dict):
                continue
            ptype=part.get("type")
            text=part.get("text")
            if ptype in ("output_text", "text") and text:
                return True
    return False
sys.exit(0 if has_text(items) else 1)
'
      then
        FOLLOWUP_REF="${ref}"
        break
      fi
    fi
  done
  if [ -n "${TOOL_OUTPUT_REF}" ] && [ -n "${FOLLOWUP_REF}" ]; then
    break
  fi
  sleep 1
done

if [ -z "${TOOL_OUTPUT_REF}" ]; then
  fail "Tool output not found in assistant messages."
fi

if [ -z "${FOLLOWUP_REF}" ]; then
  fail "LLM follow-up response not found after tool output."
fi

echo "Smoke test passed: tool call, tool output, and follow-up LLM response detected for ${CHAT_ID}."
popd >/dev/null
