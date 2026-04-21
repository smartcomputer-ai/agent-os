#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
version="${SMOLVM_VERSION:-}"

if [[ -z "$version" ]]; then
  version="$(awk -F '"' '/^version = / { print $2; exit }' "$repo_root/third_party/smolvm/Cargo.toml")"
fi

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)
    platform="darwin-arm64"
    ;;
  Linux-x86_64)
    platform="linux-x86_64"
    ;;
  *)
    echo "unsupported smolvm release platform: $(uname -s)-$(uname -m)" >&2
    exit 1
    ;;
esac

archive_name="smolvm-${version}-${platform}.tar.gz"
archive_path="$repo_root/third_party/.cache/$archive_name"
release_dir="$repo_root/third_party/smolvm-release"
url="https://github.com/smol-machines/smolvm/releases/download/v${version}/${archive_name}"

mkdir -p "$repo_root/third_party/.cache"
rm -rf "$release_dir"

if [[ ! -f "$archive_path" ]]; then
  echo "downloading $url"
  curl -fL "$url" -o "$archive_path"
else
  echo "using cached $archive_path"
fi

mkdir -p "$release_dir"
tar -xzf "$archive_path" -C "$release_dir" --strip-components=1

if [[ ! -f "$release_dir/lib/libkrun.dylib" && ! -f "$release_dir/lib/libkrun.so" ]]; then
  echo "release did not contain libkrun under $release_dir/lib" >&2
  exit 1
fi

echo "smolvm release staged at $release_dir"
