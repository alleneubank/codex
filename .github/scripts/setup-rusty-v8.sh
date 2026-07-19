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
binding_dir="${runner_temp%/}/rusty_v8/${release_tag}/${target}"
profile="ptrcomp_sandbox_release"
archive_name="librusty_v8_${profile}_${target}.a.gz"
binding_name="src_binding_${profile}_${target}.rs"
checksums_name="rusty_v8_${profile}_${target}.sha256"
trusted_checksums="${repo_root}/third_party/v8/rusty_v8_${version//./_}_release_manifests.sha256"

hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

verify_trusted_manifest() {
  [[ -f "${trusted_checksums}" ]] || {
    echo "trusted checksum manifest is missing: ${trusted_checksums}" >&2
    return 1
  }
  local expected_manifest_checksum
  expected_manifest_checksum="$(awk -v name="${checksums_name}" '$2 == name { print $1; exit }' "${trusted_checksums}")"
  [[ -n "${expected_manifest_checksum}" ]] || {
    echo "trusted checksum manifest has no entry for ${checksums_name}" >&2
    return 1
  }
  local actual_manifest_checksum
  actual_manifest_checksum="$(hash_file "${binding_dir}/${checksums_name}")"
  [[ "${actual_manifest_checksum}" == "${expected_manifest_checksum}" ]] || {
    echo "trusted checksum manifest mismatch for ${checksums_name}: expected ${expected_manifest_checksum}, got ${actual_manifest_checksum}" >&2
    return 1
  }
}

verify_binding_inputs() {
  [[ -f "${binding_dir}/${archive_name}" ]] || return 1
  [[ -f "${binding_dir}/${binding_name}" ]] || return 1
  [[ -f "${binding_dir}/${checksums_name}" ]] || return 1
  verify_trusted_manifest || return 1
  [[ "$(wc -l <"${binding_dir}/${checksums_name}" | tr -d ' ')" == "2" ]] || return 1
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "${binding_dir}" && tr -d '\r' <"${checksums_name}" | sha256sum -c -)
  else
    (cd "${binding_dir}" && tr -d '\r' <"${checksums_name}" | shasum -a 256 -c -)
  fi
}

mkdir -p "${binding_dir}"
if verify_binding_inputs >/dev/null 2>&1; then
  echo "Using cached ${release_tag} inputs for ${target}"
else
  curl -fsSL "${base_url}/${checksums_name}" -o "${binding_dir}/${checksums_name}"
  verify_trusted_manifest
  if [[ "$(wc -l <"${binding_dir}/${checksums_name}" | tr -d ' ')" != "2" ]]; then
    echo "Expected exactly two checksums in ${checksums_name}" >&2
    exit 1
  fi
  curl -fsSL "${base_url}/${archive_name}" -o "${binding_dir}/${archive_name}"
  curl -fsSL "${base_url}/${binding_name}" -o "${binding_dir}/${binding_name}"
  verify_binding_inputs
fi
{
  echo "RUSTY_V8_ARCHIVE=${binding_dir}/${archive_name}"
  echo "RUSTY_V8_SRC_BINDING_PATH=${binding_dir}/${binding_name}"
} >> "${github_env}"
