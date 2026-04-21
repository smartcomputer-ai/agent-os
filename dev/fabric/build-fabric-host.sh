#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
profile="debug"

export LIBKRUN_BUNDLE="${LIBKRUN_BUNDLE:-$repo_root/third_party/smolvm-release/lib}"
export DYLD_LIBRARY_PATH="${DYLD_LIBRARY_PATH:-$repo_root/third_party/smolvm-release/lib}"
export SMOLVM_AGENT_ROOTFS="${SMOLVM_AGENT_ROOTFS:-$repo_root/third_party/smolvm-release/agent-rootfs}"

for arg in "$@"; do
  case "$arg" in
    --release)
      profile="release"
      ;;
  esac
done

cargo build -p fabric-host --features smolvm-runtime "$@"

if [[ "$(uname -s)" != "Darwin" ]]; then
  exit 0
fi

bin="$repo_root/target/$profile/fabric-host"
entitlements="$repo_root/third_party/smolvm/smolvm.entitlements"

if [[ ! -f "$entitlements" ]]; then
  echo "missing smolvm entitlements: $entitlements" >&2
  exit 1
fi

codesign --force --sign - --options runtime --entitlements "$entitlements" "$bin"
echo "signed $bin"
