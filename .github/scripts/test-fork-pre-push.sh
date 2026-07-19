#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
hook="${repo_root}/.githooks/pre-push"
test_root="$(mktemp -d)"
trap 'rm -rf "${test_root}"' EXIT

fail() {
  echo "$*" >&2
  exit 1
}

upstream="${test_root}/upstream"
source_repo="${test_root}/source"
git init --quiet "${upstream}"
git -C "${upstream}" config user.name "Fork Hook Test"
git -C "${upstream}" config user.email "fork-hook-test@example.com"
git -C "${upstream}" commit --quiet --allow-empty -m fixture
git -C "${upstream}" tag rust-v0.144.6

git init --quiet --initial-branch=main "${source_repo}"
git -C "${source_repo}" config user.name "Fork Hook Test"
git -C "${source_repo}" config user.email "fork-hook-test@example.com"
mkdir -p "${source_repo}/codex-rs"
printf '%s\n' 0.144.6 >"${source_repo}/codex-rs/fork-version.txt"
git -C "${source_repo}" add codex-rs/fork-version.txt
git -C "${source_repo}" commit --quiet -m current
current_sha="$(git -C "${source_repo}" rev-parse HEAD)"

printf '%s\n' 0.144.5 >"${source_repo}/codex-rs/fork-version.txt"
git -C "${source_repo}" commit --quiet -am stale
stale_sha="$(git -C "${source_repo}" rev-parse HEAD)"
zero_sha="0000000000000000000000000000000000000000"

run_hook() {
  local update="$1"
  CODEX_FORK_REPO_ROOT="${source_repo}" \
    CODEX_FORK_VERSION_SCRIPT="${repo_root}/.github/scripts/fork-version.sh" \
    CODEX_FORK_VERSION_UPSTREAM="${upstream}" \
    bash "${hook}" origin git@example.com:fork.git <<<"${update}"
}

run_hook "refs/heads/main ${current_sha} refs/heads/main ${zero_sha}" >/dev/null

stale_output=""
if stale_output="$(run_hook "refs/heads/main ${stale_sha} refs/heads/main ${current_sha}" 2>&1)"; then
  fail "pre-push accepted a stale pin from the pushed commit"
fi
[[ "${stale_output}" == *"0.144.5 is stale"* ]] || fail "Unexpected stale error: ${stale_output}"

wrong_source_output=""
if wrong_source_output="$(run_hook "refs/heads/topic ${current_sha} refs/heads/main ${zero_sha}" 2>&1)"; then
  fail "pre-push accepted a non-main source for origin/main"
fi
[[ "${wrong_source_output}" == *"local main"* ]] ||
  fail "Unexpected source-branch error: ${wrong_source_output}"

fork_output=""
if fork_output="$(run_hook "refs/heads/main ${current_sha} refs/heads/fork ${zero_sha}" 2>&1)"; then
  fail "pre-push accepted recreation of origin/fork"
fi
[[ "${fork_output}" == *"origin/fork is retired"* ]] ||
  fail "Unexpected fork-branch error: ${fork_output}"

run_hook "(delete) ${zero_sha} refs/heads/fork ${current_sha}" >/dev/null

delete_main_output=""
if delete_main_output="$(run_hook "(delete) ${zero_sha} refs/heads/main ${current_sha}" 2>&1)"; then
  fail "pre-push accepted deletion of origin/main"
fi
[[ "${delete_main_output}" == *"cannot be deleted"* ]] ||
  fail "Unexpected main-deletion error: ${delete_main_output}"

echo "fork pre-push contract passed"
