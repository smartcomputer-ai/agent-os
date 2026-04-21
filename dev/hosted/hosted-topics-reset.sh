#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEV_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
COMPOSE_FILE="${DEV_DIR}/docker-compose.yaml"

INGRESS_TOPIC="${AOS_KAFKA_INGRESS_TOPIC:-aos-ingress}"
JOURNAL_TOPIC="${AOS_KAFKA_JOURNAL_TOPIC:-aos-journal}"
PROJECTION_TOPIC="${AOS_KAFKA_PROJECTION_TOPIC:-aos-projection}"

delete_topic() {
  local topic="$1"
  if docker compose -f "${COMPOSE_FILE}" exec -T redpanda rpk topic describe "${topic}" >/dev/null 2>&1; then
    docker compose -f "${COMPOSE_FILE}" exec -T redpanda rpk topic delete "${topic}"
  fi
}

delete_topic "${INGRESS_TOPIC}"
delete_topic "${JOURNAL_TOPIC}"
delete_topic "${PROJECTION_TOPIC}"

docker compose -f "${COMPOSE_FILE}" exec -T redpanda \
  rpk topic delete -r 'aos-.*' >/dev/null 2>&1 || true

"${SCRIPT_DIR}/hosted-topics-ensure.sh"
