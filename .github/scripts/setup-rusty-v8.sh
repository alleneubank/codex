#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: setup-rusty-v8.sh <rust-target>" >&2
}

if [[ $# -ne 1 ]]; then
  usage
  exit 2
fi
target="$1"
repo_root="${GITHUB_WORKSPACE:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
runner_temp="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
github_env="${GITHUB_ENV:-${runner_temp}/codex-rusty-v8.env}"
version="$(python3 "${repo_root}/.github/scripts/rusty_v8_bazel.py" resolved-v8-crate-version)"
release_tag="rusty-v8-v${version}"
base_url="https://github.com/openai/codex/releases/download/${release_tag}"
binding_dir="${runner_temp%/}/rusty_v8/${target}"
archive_name="librusty_v8_release_${target}.a.gz"
binding_name="src_binding_release_${target}.rs"
checksums_name="rusty_v8_release_${target}.sha256"
mkdir -p "${binding_dir}"
curl -fsSL "${base_url}/${archive_name}" -o "${binding_dir}/${archive_name}"
curl -fsSL "${base_url}/${binding_name}" -o "${binding_dir}/${binding_name}"
curl -fsSL "${base_url}/${checksums_name}" -o "${binding_dir}/${checksums_name}"
if [[ "$(wc -l < "${binding_dir}/${checksums_name}" | tr -d ' ')" -ne 2 ]]; then
  echo "Expected exactly two checksums in ${checksums_name}" >&2
  exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
  (cd "${binding_dir}" && sha256sum -c "${checksums_name}")
else
  (cd "${binding_dir}" && shasum -a 256 -c "${checksums_name}")
fi
{
  echo "RUSTY_V8_ARCHIVE=${binding_dir}/${archive_name}"
  echo "RUSTY_V8_SRC_BINDING_PATH=${binding_dir}/${binding_name}"
} >> "${github_env}"
