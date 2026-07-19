#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
release_script="${repo_root}/.github/scripts/fork-release.sh"
test_date="20000102"
version="$(tr -d '[:space:]' < "${repo_root}/codex-rs/fork-version.txt")"
test_root="$(mktemp -d)"
trap 'rm -rf "${test_root}"' EXIT
test_upstream="${test_root}/upstream"
source_repo="${test_root}/source"

git init --quiet "${test_upstream}"
git -C "${test_upstream}" config user.name "Fork Release Test"
git -C "${test_upstream}" config user.email "fork-release-test@example.com"
git -C "${test_upstream}" commit --quiet --allow-empty -m fixture
git -C "${test_upstream}" tag "rust-v${version}"

git init --quiet --initial-branch=main "${source_repo}"
git -C "${source_repo}" config user.name "Fork Release Test"
git -C "${source_repo}" config user.email "fork-release-test@example.com"
git -C "${source_repo}" commit --quiet --allow-empty -m source

export CODEX_FORK_RELEASE_GIT_ROOT="${source_repo}"
export CODEX_FORK_VERSION_UPSTREAM="${test_upstream}"

bash "${repo_root}/.github/scripts/test-fork-version.sh"

help_output="$(${release_script} help)"
if [[ "${help_output}" != *"publish --publish"* ]] \
  || [[ "${help_output}" != *"without rebuilding"* ]] \
  || [[ "${help_output}" != *"origin/main"* ]]; then
  echo "release help did not describe the staged main-branch publication path" >&2
  exit 1
fi

short_sha="$(git -C "${source_repo}" rev-parse --short=9 HEAD)"
expected_tag="rust-v${version}-fork.${test_date}.g${short_sha}"
actual_tag="$(${release_script} metadata --date "${test_date}")"
if [[ "${actual_tag}" != "${expected_tag}" ]]; then
  echo "Expected ${expected_tag}, got ${actual_tag}" >&2
  exit 1
fi

git -C "${source_repo}" switch --quiet -c topic
branch_output=""
if branch_output="$(${release_script} metadata --date "${test_date}" 2>&1)"; then
  echo "release metadata accepted a non-main branch" >&2
  exit 1
fi
if [[ "${branch_output}" != *"local main"* ]]; then
  echo "Unexpected branch error: ${branch_output}" >&2
  exit 1
fi
git -C "${source_repo}" switch --quiet main

roll_output="$(${release_script} roll --date "${test_date}")"
if [[ "${roll_output}" != *"${expected_tag}"* ]] \
  || [[ "${roll_output}" != *"Apple Silicon macOS"* ]] \
  || [[ "${roll_output}" != *"publish --publish --date ${test_date}"* ]]; then
  echo "roll dry-run did not describe the candidate, host, and date-stable publish handoff" >&2
  exit 1
fi
roll_publish_output=""
if roll_publish_output="$(
  "${release_script}" roll --publish --date "${test_date}" 2>&1
)" || [[ "${roll_publish_output}" != *"--run"* ]]; then
  echo "roll did not fail closed when --publish omitted --run" >&2
  exit 1
fi

if "${release_script}" metadata --date 2000-01-02 >/dev/null 2>&1; then
  echo "metadata accepted a non-YYYYMMDD date" >&2
  exit 1
fi
publish_output=""
if publish_output="$(${release_script} publish 2>&1)" \
  || [[ "${publish_output}" != *"--publish"* ]]; then
  echo "publish did not fail closed without the literal --publish flag" >&2
  exit 1
fi
empty_output="${test_root}/empty-output"
mkdir -p "${empty_output}"
verify_output=""
if verify_output="$(${release_script} verify --output-dir "${empty_output}" 2>&1)" \
  || [[ "${verify_output}" != *"Missing release bundle"* ]]; then
  echo "verify did not fail closed for an empty artifact directory" >&2
  exit 1
fi

IFS=. read -r major minor patch <<<"${version}"
git -C "${test_upstream}" tag "rust-v${major}.${minor}.$((patch + 1))"
stale_output=""
if stale_output="$(${release_script} metadata --date "${test_date}" 2>&1)" \
  || [[ "${stale_output}" != *"is stale"* ]]; then
  echo "release metadata did not fail closed for a stale fork version" >&2
  exit 1
fi
echo "fork release script contract passed"
