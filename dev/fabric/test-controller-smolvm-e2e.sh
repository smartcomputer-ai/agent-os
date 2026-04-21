#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

"$repo_root/dev/fabric/bootstrap-smolvm-release.sh"

export FABRIC_HOST_BIN="${FABRIC_HOST_BIN:-$repo_root/target/debug/fabric-host}"
export FABRIC_SMOLVM_E2E="${FABRIC_SMOLVM_E2E:-1}"
export LIBKRUN_BUNDLE="${LIBKRUN_BUNDLE:-$repo_root/third_party/smolvm-release/lib}"
export DYLD_LIBRARY_PATH="${DYLD_LIBRARY_PATH:-$repo_root/third_party/smolvm-release/lib}"
export SMOLVM_AGENT_ROOTFS="${SMOLVM_AGENT_ROOTFS:-$repo_root/third_party/smolvm-release/agent-rootfs}"

test_bin="$(
  cargo test -p fabric-controller --test controller_smolvm_e2e --no-run 2>&1 \
    | tee /dev/stderr \
    | sed -n 's/^  Executable tests\/controller_smolvm_e2e.rs (\(.*\))$/\1/p' \
    | tail -n 1
)"

if [[ -z "$test_bin" ]]; then
  echo "failed to locate compiled controller_smolvm_e2e test binary" >&2
  exit 1
fi

if [[ "$test_bin" != /* ]]; then
  test_bin="$repo_root/$test_bin"
fi

if [[ ! -x "$test_bin" ]]; then
  echo "compiled controller_smolvm_e2e test binary is not executable: $test_bin" >&2
  exit 1
fi

cargo build -p fabric-controller
"$repo_root/dev/fabric/build-fabric-host.sh"

"$test_bin" --nocapture "$@"
