#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
HOSTED_STATE_DIR="${REPO_ROOT}/.aos-hosted"

"${SCRIPT_DIR}/hosted-topics-reset.sh"
"${SCRIPT_DIR}/hosted-blobstore-reset.sh"

if [[ -d "${HOSTED_STATE_DIR}" ]]; then
  rm -rf "${HOSTED_STATE_DIR}"
  echo "removed hosted state dir: ${HOSTED_STATE_DIR}"
else
  echo "hosted state dir already absent: ${HOSTED_STATE_DIR}"
fi

echo "hosted reset complete"
