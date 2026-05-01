#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Bootstrap the shared repo Python environment and install the local aos_harness package.

Usage:
  ./setup-python.sh [--recreate]

Options:
  --recreate   Delete the existing repo .venv before creating it again.
EOF
}

recreate=0
while (($# > 0)); do
  case "$1" in
    --recreate)
      recreate=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -f "${script_dir}/Cargo.toml" && -f "${script_dir}/crates/aos-harness-py/Cargo.toml" ]]; then
  repo_root="${script_dir}"
elif [[ -f "${script_dir}/../Cargo.toml" && -f "${script_dir}/../crates/aos-harness-py/Cargo.toml" ]]; then
  repo_root="$(cd "${script_dir}/.." && pwd)"
else
  echo "could not determine repo root from ${BASH_SOURCE[0]}" >&2
  exit 1
fi

venv_dir="${repo_root}/.venv"

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required" >&2
  exit 1
fi

create_venv() {
  if python3 -m venv "${venv_dir}" 2>/tmp/aos-setup-python-venv.err; then
    return 0
  fi

  echo "python3 -m venv failed:" >&2
  sed 's/^/  /' /tmp/aos-setup-python-venv.err >&2 || true

  if command -v uv >/dev/null 2>&1; then
    echo "falling back to: uv venv --seed ${venv_dir}" >&2
    uv venv --seed "${venv_dir}"
    return 0
  fi

  cat >&2 <<'EOF'
could not create a Python environment with pip.

Install python3-venv/ensurepip for your system Python, or install uv and rerun:
  curl -LsSf https://astral.sh/uv/install.sh | sh
EOF
  return 1
}

venv_has_pip() {
  [[ -x "${venv_dir}/bin/python" ]] && "${venv_dir}/bin/python" -m pip --version >/dev/null 2>&1
}

if (( recreate == 1 )) && [[ -d "${venv_dir}" ]]; then
  rm -rf "${venv_dir}"
fi

if [[ ! -d "${venv_dir}" ]]; then
  create_venv
elif ! venv_has_pip; then
  echo "${venv_dir} exists but pip is missing; recreating it" >&2
  rm -rf "${venv_dir}"
  create_venv
fi

# shellcheck disable=SC1091
source "${venv_dir}/bin/activate"

python -m pip install --upgrade pip maturin pytest
python -m maturin develop --manifest-path "${repo_root}/crates/aos-harness-py/Cargo.toml"

cat <<EOF
Repo Python environment is ready.

Activate it with:
  source ${venv_dir}/bin/activate

Reinstall updated local bindings after Rust/Python bridge changes with:
  source ${venv_dir}/bin/activate
  python -m maturin develop --manifest-path ${repo_root}/crates/aos-harness-py/Cargo.toml
EOF
