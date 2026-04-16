#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
NODE_STATE_DIR="${REPO_ROOT}/.aos-node"

"${SCRIPT_DIR}/hosted-topics-reset.sh"
"${SCRIPT_DIR}/hosted-blobstore-reset.sh"

if [[ -d "${NODE_STATE_DIR}" ]]; then
  rm -rf "${NODE_STATE_DIR}"
  echo "removed node state dir: ${NODE_STATE_DIR}"
else
  echo "node state dir already absent: ${NODE_STATE_DIR}"
fi

echo "node reset complete"
