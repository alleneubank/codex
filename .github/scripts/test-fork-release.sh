#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
release_script="${repo_root}/.github/scripts/fork-release.sh"
test_date="20000102"
version="$(tr -d '[:space:]' < "${repo_root}/codex-rs/fork-version.txt")"
short_sha="$(git -C "${repo_root}" rev-parse --short=9 HEAD)"
expected_tag="rust-v${version}-fork.${test_date}.g${short_sha}"
actual_tag="$(${release_script} metadata --date "${test_date}")"
if [[ "${actual_tag}" != "${expected_tag}" ]]; then
  echo "Expected ${expected_tag}, got ${actual_tag}" >&2
  exit 1
fi

roll_output="$(${release_script} roll --date "${test_date}")"
if [[ "${roll_output}" != *"${expected_tag}"* ]] \
  || [[ "${roll_output}" != *"Apple Silicon macOS"* ]]; then
  echo "roll dry-run did not describe the candidate tag and required release host" >&2
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
empty_output="$(mktemp -d)"
trap 'rm -rf "${empty_output}"' EXIT
verify_output=""
if verify_output="$(${release_script} verify --output-dir "${empty_output}" 2>&1)" \
  || [[ "${verify_output}" != *"Missing release bundle"* ]]; then
  echo "verify did not fail closed for an empty artifact directory" >&2
  exit 1
fi
echo "fork release script contract passed"
