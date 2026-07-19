#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: build-fork-release-target.sh <rust-target> <output-dir>" >&2
}

if [[ $# -ne 2 ]]; then
  usage
  exit 2
fi
target="$1"
output_dir="$2"

repo_root="${GITHUB_WORKSPACE:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
codex_root="${repo_root}/codex-rs"
runner_temp="${RUNNER_TEMP:-${TMPDIR:-/tmp}/codex-fork-release}"
github_env="${GITHUB_ENV:-${runner_temp%/}/build.env}"
cargo_target_dir="${CARGO_TARGET_DIR:-${codex_root}/target/fork-release}"
release_dir="${cargo_target_dir}/${target}/release"
bundle="${output_dir%/}/codex-${target}-bundle.tar.zst"
version="$(tr -d '[:space:]' < "${codex_root}/fork-version.txt")"
revision="$(git -C "${repo_root}" rev-parse --short=12 HEAD)"
expected_version="codex-cli ${version}+fork.${revision}"
bundle_entries=(codex codex-code-mode-host)

case "${target}" in
  aarch64-apple-darwin)
    if [[ "$(uname -s)" != Darwin || "$(uname -m)" != arm64 ]]; then
      echo "${target} must be built on a native macOS arm64 host" >&2
      exit 1
    fi
    include_bwrap="false"
    ;;
  x86_64-unknown-linux-musl)
    if [[ "$(uname -s)" != Linux || "$(uname -m)" != x86_64 ]]; then
      echo "${target} must be built in a native or translated Linux x86_64 environment" >&2
      exit 1
    fi
    include_bwrap="true"
    ;;
  *)
    echo "Unsupported fork release target: ${target}" >&2
    exit 2
    ;;
esac

mkdir -p "${runner_temp}" "${output_dir}"
: > "${github_env}"

if [[ "${include_bwrap}" == true ]]; then
  TARGET="${target}" GITHUB_ENV="${github_env}" RUNNER_TEMP="${runner_temp}" \
    bash "${repo_root}/.github/scripts/install-musl-build-tools.sh"
fi
GITHUB_WORKSPACE="${repo_root}" GITHUB_ENV="${github_env}" RUNNER_TEMP="${runner_temp}" \
  bash "${repo_root}/.github/scripts/setup-rusty-v8.sh" "${target}"

# GITHUB_ENV values may contain spaces, so import them as data rather than
# evaluating the file as shell source.
while IFS='=' read -r key value; do
  if [[ ! "${key}" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
    echo "Invalid environment key from release setup: ${key}" >&2
    exit 1
  fi
  export "${key}=${value}"
done < "${github_env}"
export CARGO_NET_GIT_FETCH_WITH_CLI=true
export CARGO_TARGET_DIR="${cargo_target_dir}"
export AWS_LC_SYS_NO_JITTER_ENTROPY=1
export AWS_LC_SYS_NO_JITTER_ENTROPY_x86_64_unknown_linux_musl=1

if [[ "${include_bwrap}" == true ]]; then
  (
    cd "${codex_root}"
    cargo build --locked --target "${target}" --release --timings --bin bwrap
  )
  bwrap_path="${release_dir}/bwrap"
  strip --strip-debug --strip-unneeded "${bwrap_path}"
  if command -v sha256sum >/dev/null 2>&1; then
    CODEX_BWRAP_SHA256="$(sha256sum "${bwrap_path}" | awk '{print $1}')"
  else
    CODEX_BWRAP_SHA256="$(shasum -a 256 "${bwrap_path}" | awk '{print $1}')"
  fi
  export CODEX_BWRAP_SHA256
  echo "Built bwrap with sha256:${CODEX_BWRAP_SHA256}"
fi

(
  cd "${codex_root}"
  cargo build \
    --locked \
    --target "${target}" \
    --release \
    --timings \
    --bin codex \
    --bin codex-code-mode-host
)

if [[ "${target}" == *apple-darwin ]]; then
  for binary in codex codex-code-mode-host; do
    binary_path="${release_dir}/${binary}"
    strip -S -x "${binary_path}"
  done
else
  strip --strip-debug --strip-unneeded \
    "${release_dir}/codex" \
    "${release_dir}/codex-code-mode-host"
fi

bundle_root="${runner_temp%/}/codex-${target}-bundle"
rm -rf "${bundle_root}"
mkdir -p "${bundle_root}"
install -m 0755 "${release_dir}/codex" "${bundle_root}/codex"
install -m 0755 "${release_dir}/codex-code-mode-host" "${bundle_root}/codex-code-mode-host"
if [[ "${include_bwrap}" == true ]]; then
  mkdir -p "${bundle_root}/codex-resources"
  install -m 0755 "${release_dir}/bwrap" "${bundle_root}/codex-resources/bwrap"
  bundle_entries+=(codex-resources/bwrap)
fi

rm -f "${bundle}"
tar -C "${bundle_root}" -cf - "${bundle_entries[@]}" | zstd -T0 -19 -o "${bundle}"
bash "${repo_root}/.github/scripts/verify-fork-release-bundle.sh" \
  "${target}" "${bundle}" "${expected_version}"

if command -v sha256sum >/dev/null 2>&1; then
  sha256sum "${bundle}"
else
  shasum -a 256 "${bundle}"
fi
