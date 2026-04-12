#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEV_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
COMPOSE_FILE="${DEV_DIR}/docker-compose.yaml"

docker compose -f "${COMPOSE_FILE}" up -d redpanda minio console

echo "Waiting for Redpanda..."
until docker compose -f "${COMPOSE_FILE}" exec -T redpanda rpk cluster info >/dev/null 2>&1; do
  sleep 1
done

echo "Waiting for MinIO..."
until "${SCRIPT_DIR}/hosted-blobstore-ensure.sh" >/dev/null 2>&1; do
  sleep 1
done

"${SCRIPT_DIR}/hosted-topics-ensure.sh"

cat <<'EOF'

Hosted local infra is up.

Kafka:
  bootstrap servers: localhost:19092
  console:           http://localhost:8080
  topics:            aos-ingress, aos-journal, aos-projection

Blobstore:
  S3 endpoint:       http://localhost:19000
  MinIO console:     http://localhost:19001

Suggested env:
  export AOS_KAFKA_BOOTSTRAP_SERVERS=localhost:19092
  export AOS_KAFKA_PROJECTION_TOPIC=aos-projection
  export AOS_BLOBSTORE_BUCKET=aos-dev
  export AOS_BLOBSTORE_ENDPOINT=http://localhost:19000
  export AOS_BLOBSTORE_REGION=us-east-1
  export AOS_BLOBSTORE_PREFIX=aos
  export AOS_BLOBSTORE_FORCE_PATH_STYLE=true
  export AOS_PARTITION_COUNT=1
  export AWS_ACCESS_KEY_ID=minioadmin
  export AWS_SECRET_ACCESS_KEY=minioadmin
EOF
