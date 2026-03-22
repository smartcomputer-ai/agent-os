#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEV_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
COMPOSE_FILE="${DEV_DIR}/docker-compose.yaml"

INGRESS_TOPIC="${AOS_KAFKA_INGRESS_TOPIC:-aos-ingress}"
JOURNAL_TOPIC="${AOS_KAFKA_JOURNAL_TOPIC:-aos-journal}"
PROJECTION_TOPIC="${AOS_KAFKA_PROJECTION_TOPIC:-aos-projection}"
PARTITIONS="${AOS_PARTITION_COUNT:-${AOS_HOSTED_PARTITIONS:-1}}"
REPLICAS="${AOS_HOSTED_REPLICAS:-1}"

topic_exists() {
  docker compose -f "${COMPOSE_FILE}" exec -T redpanda \
    rpk topic describe "$1" >/dev/null 2>&1
}

topic_partition_count() {
  docker compose -f "${COMPOSE_FILE}" exec -T redpanda \
    rpk topic describe "$1" -p | awk '/^[0-9]+[[:space:]]/ { count += 1 } END { print count + 0 }'
}

delete_topic() {
  local topic="$1"
  if ! topic_exists "${topic}"; then
    return 0
  fi
  docker compose -f "${COMPOSE_FILE}" exec -T redpanda \
    rpk topic delete "${topic}" >/dev/null
  for _ in {1..20}; do
    if ! topic_exists "${topic}"; then
      return 0
    fi
    sleep 0.5
  done
  echo "timed out waiting for topic deletion: ${topic}" >&2
  return 1
}

create_topic() {
  local topic="$1"
  shift
  docker compose -f "${COMPOSE_FILE}" exec -T redpanda \
    rpk topic create "${topic}" "$@"
}

ensure_topic() {
  local topic="$1"
  shift || true
  if topic_exists "${topic}"; then
    local actual_partitions
    actual_partitions="$(topic_partition_count "${topic}")"
    if [[ "${actual_partitions}" == "${PARTITIONS}" ]]; then
      echo "topic ok: ${topic} partitions=${actual_partitions}"
      return 0
    fi
    echo "recreating topic ${topic}: partitions ${actual_partitions} -> ${PARTITIONS}"
    delete_topic "${topic}"
  else
    echo "creating topic ${topic}: partitions=${PARTITIONS}"
  fi
  create_topic "${topic}" --partitions "${PARTITIONS}" --replicas "${REPLICAS}" "$@"
}

ensure_topic "${INGRESS_TOPIC}"
ensure_topic "${JOURNAL_TOPIC}"
ensure_topic "${PROJECTION_TOPIC}" --topic-config cleanup.policy=compact

echo "topics ensured:"
echo "  ingress=${INGRESS_TOPIC}"
echo "  journal=${JOURNAL_TOPIC}"
echo "  projection=${PROJECTION_TOPIC} (compacted)"
echo "  partitions=${PARTITIONS}"
