#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
version_script="${repo_root}/.github/scripts/fork-version.sh"
test_root="$(mktemp -d)"
trap 'rm -rf "${test_root}"' EXIT

fail() {
  echo "$*" >&2
  exit 1
}

create_upstream() {
  local path="$1"
  shift
  git init --quiet "${path}"
  git -C "${path}" config user.name "Fork Version Test"
  git -C "${path}" config user.email "fork-version-test@example.com"
  git -C "${path}" commit --quiet --allow-empty -m "fixture"
  local tag
  for tag in "$@"; do
    git -C "${path}" tag "${tag}"
  done
}

upstream="${test_root}/upstream"
version_file="${test_root}/fork-version.txt"
create_upstream \
  "${upstream}" \
  rust-v0.144.1 \
  rust-v0.144.4 \
  rust-v0.9.99 \
  rust-v0.145.0-alpha.12 \
  rust-v0.144.4-fork.20000102.gdeadbeef0 \
  rust-v0.144 \
  rust-v00.145.0
printf '%s\n' "0.144.1" >"${version_file}"

resolved="$("${version_script}" resolve --upstream "${upstream}" --version-file "${version_file}")"
[[ "${resolved}" == "0.144.4" ]] || fail "Expected 0.144.4, got ${resolved}"

stale_output=""
if stale_output="$("${version_script}" check --upstream "${upstream}" --version-file "${version_file}" 2>&1)"; then
  fail "check accepted stale fork version"
fi
[[ "${stale_output}" == *"0.144.1 is stale"* ]] || fail "Unexpected stale error: ${stale_output}"

"${version_script}" update --upstream "${upstream}" --version-file "${version_file}" >/dev/null
[[ "$(tr -d '[:space:]' <"${version_file}")" == "0.144.4" ]] ||
  fail "update did not write 0.144.4"
"${version_script}" check --upstream "${upstream}" --version-file "${version_file}" >/dev/null

printf '%s\n' "not-semver" >"${version_file}"
invalid_output=""
if invalid_output="$("${version_script}" check --upstream "${upstream}" --version-file "${version_file}" 2>&1)"; then
  fail "check accepted an invalid pin"
fi
[[ "${invalid_output}" == *"must contain stable SemVer"* ]] ||
  fail "Unexpected invalid-pin error: ${invalid_output}"

prerelease_only="${test_root}/prerelease-only"
create_upstream "${prerelease_only}" rust-v0.145.0-alpha.12
no_stable_output=""
if no_stable_output="$("${version_script}" resolve --upstream "${prerelease_only}" --version-file "${version_file}" 2>&1)"; then
  fail "resolve accepted an upstream without stable tags"
fi
[[ "${no_stable_output}" == *"no stable rust-vX.Y.Z tags"* ]] ||
  fail "Unexpected no-stable error: ${no_stable_output}"

remote_output=""
if remote_output="$("${version_script}" resolve --upstream "${test_root}/missing" --version-file "${version_file}" 2>&1)"; then
  fail "resolve accepted an unreachable upstream"
fi
[[ "${remote_output}" == *"unable to query upstream tags"* ]] ||
  fail "Unexpected upstream-query error: ${remote_output}"

echo "fork version resolver contract passed"
