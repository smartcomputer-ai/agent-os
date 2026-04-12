#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEV_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
COMPOSE_FILE="${DEV_DIR}/docker-compose.yaml"

BUCKET="${AOS_BLOBSTORE_BUCKET:-aos-dev}"
MINIO_ROOT_USER="${MINIO_ROOT_USER:-minioadmin}"
MINIO_ROOT_PASSWORD="${MINIO_ROOT_PASSWORD:-minioadmin}"

"${SCRIPT_DIR}/hosted-blobstore-ensure.sh"

docker compose -f "${COMPOSE_FILE}" run --rm --no-deps \
  -e MC_HOST_local="http://${MINIO_ROOT_USER}:${MINIO_ROOT_PASSWORD}@minio:9000" \
  mc rm --recursive --force "local/${BUCKET}"

echo "blobstore bucket contents reset: s3://${BUCKET}"
