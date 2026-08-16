#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
release_script="${repo_root}/.github/scripts/fork-release.sh"
test_date="20000102"
version="$(tr -d '[:space:]' < "${repo_root}/codex-rs/fork-version.txt")"
test_root="$(mktemp -d)"
trap 'rm -rf "${test_root}"' EXIT
test_upstream="${test_root}/upstream"
test_origin="${test_root}/origin.git"
source_repo="${test_root}/source"
fake_bin="${test_root}/fake-bin"
fake_git_log="${test_root}/fake-git.log"
fake_gh_log="${test_root}/fake-gh.log"
fake_docker_log="${test_root}/fake-docker.log"
fake_verify_log="${test_root}/fake-verify.log"
real_git="$(command -v git)"
real_bash="$(command -v bash)"
real_uname="$(command -v uname)"
real_file="$(command -v file)"
real_cc="$(command -v cc)"

git init --quiet "${test_upstream}"
git -C "${test_upstream}" config user.name "Fork Release Test"
git -C "${test_upstream}" config user.email "fork-release-test@example.com"
git -C "${test_upstream}" commit --quiet --allow-empty -m fixture
git -C "${test_upstream}" tag "rust-v${version}"

git init --quiet --initial-branch=main "${source_repo}"
git -C "${source_repo}" config user.name "Fork Release Test"
git -C "${source_repo}" config user.email "fork-release-test@example.com"
git -C "${source_repo}" commit --quiet --allow-empty -m source
git init --quiet --bare "${test_origin}"
git -C "${source_repo}" remote add origin "${test_origin}"
git -C "${source_repo}" push --quiet origin main

mkdir -p "${fake_bin}"
cat >"${fake_bin}/git" <<EOF
#!/usr/bin/env bash
set -euo pipefail
is_push=false
for arg in "\$@"; do
  if [[ "\${arg}" == push ]]; then
    is_push=true
  fi
done
if [[ "\${is_push}" == true && "\$*" == *"${source_repo}"* ]]; then
  printf '%s\n' "\$*" >>"${fake_git_log}"
  if [[ "\${FAKE_REJECT_MAIN_LEASE:-false}" == true \
    && "\$*" == *"--force-with-lease=refs/heads/main:"* ]]; then
    echo "simulated force-with-lease rejection" >&2
    exit 1
  fi
  exit 0
fi
exec "${real_git}" "\$@"
EOF
cat >"${fake_bin}/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${FAKE_GH_LOG}"
case "${1:-} ${2:-}" in
  "auth status")
    exit 0
    ;;
  "api "*)
    case "${FAKE_GH_API_MODE:-404}" in
      404)
        echo "release not found (HTTP 404)" >&2
        exit 1
        ;;
      existing)
        printf '%s\n' '{"id": 1}'
        exit 0
        ;;
      error)
        echo "GitHub unavailable (HTTP 500)" >&2
        exit 1
        ;;
    esac
    ;;
  "release create")
    exit 0
    ;;
esac
echo "unexpected gh invocation: $*" >&2
exit 1
EOF
cat >"${fake_bin}/docker" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${FAKE_DOCKER_LOG}"
case "${1:-}" in
  info|build|run)
    exit 0
    ;;
esac
echo "unexpected docker invocation: $*" >&2
exit 1
EOF
cat >"${fake_bin}/uname" <<EOF
#!/usr/bin/env bash
set -euo pipefail
case "\${1:-}" in
  -s)
    printf '%s\n' Darwin
    ;;
  -m)
    printf '%s\n' arm64
    ;;
  *)
    exec "${real_uname}" "\$@"
    ;;
esac
EOF
cat >"${fake_bin}/bash" <<'EOF'
#!/bin/bash
set -euo pipefail
if [[ "${1:-}" == */verify-fork-release-bundle.sh ]]; then
  [[ "$#" -eq 4 ]]
  if [[ ! -f "$3" ]]; then
    echo "Missing release bundle: $3" >&2
    exit 1
  fi
  printf '%s\n' "$2 $3 $4" >>"${FAKE_VERIFY_LOG}"
  exit 0
fi
exec "${REAL_BASH}" "$@"
EOF
cat >"${fake_bin}/file" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${FAKE_FILE_ARCH:-}" == arm64 ]]; then
  printf '%s: Mach-O 64-bit arm64 executable\n' "$1"
  exit 0
elif [[ "${FAKE_FILE_ARCH:-}" == x86_64 ]]; then
  printf '%s: ELF 64-bit LSB pie executable, x86-64\n' "$1"
  exit 0
fi
exec "${REAL_FILE}" "$@"
EOF
chmod +x "${fake_bin}/git" "${fake_bin}/gh" "${fake_bin}/docker" "${fake_bin}/uname" "${fake_bin}/bash" "${fake_bin}/file"
export FAKE_GH_LOG="${fake_gh_log}"
export FAKE_DOCKER_LOG="${fake_docker_log}"
export FAKE_VERIFY_LOG="${fake_verify_log}"
export REAL_BASH="${real_bash}"
export REAL_FILE="${real_file}"
export PATH="${fake_bin}:${PATH}"

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

output_dir="${test_root}/bundles"
mkdir -p "${output_dir}"
fixture_root="${test_root}/fixtures"
fixture_revision="$(${real_git} -C "${source_repo}" rev-parse --short=12 HEAD)"
expected_binary_version="codex-cli ${version}+fork.${fixture_revision}"
mkdir -p "${fixture_root}/mac" "${fixture_root}/linux/codex-resources"
cat >"${fixture_root}/version-helper.c" <<'EOF'
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv) {
  if (argc != 2 || strcmp(argv[1], "--version") != 0) {
    return 1;
  }
  if (strstr(argv[0], "bwrap") != NULL) {
    printf("bubblewrap built for Codex %s\n", VERSION);
  } else {
    printf("%s\n", VERSION);
  }
  return 0;
}
EOF
"${real_cc}" -O2 "-DVERSION=\"${expected_binary_version}\"" \
  "${fixture_root}/version-helper.c" -o "${fixture_root}/version-helper"
"${real_cc}" -O2 "-DVERSION=\"${expected_binary_version}.mismatch\"" \
  "${fixture_root}/version-helper.c" -o "${fixture_root}/version-helper-mismatch"
cp "${fixture_root}/version-helper" "${fixture_root}/mac/codex"
cp "${fixture_root}/version-helper" "${fixture_root}/mac/codex-code-mode-host"
cp "${fixture_root}/version-helper" "${fixture_root}/linux/codex"
cp "${fixture_root}/version-helper" "${fixture_root}/linux/codex-code-mode-host"
cp "${fixture_root}/version-helper" "${fixture_root}/linux/codex-resources/bwrap"
chmod +x \
  "${fixture_root}/mac/codex" \
  "${fixture_root}/mac/codex-code-mode-host" \
  "${fixture_root}/linux/codex" \
  "${fixture_root}/linux/codex-code-mode-host" \
  "${fixture_root}/linux/codex-resources/bwrap"
tar --use-compress-program=zstd -cf \
  "${output_dir}/codex-aarch64-apple-darwin-bundle.tar.zst" \
  -C "${fixture_root}/mac" codex codex-code-mode-host
tar --use-compress-program=zstd -cf \
  "${output_dir}/codex-x86_64-unknown-linux-musl-bundle.tar.zst" \
  -C "${fixture_root}/linux" codex codex-code-mode-host codex-resources/bwrap

FAKE_FILE_ARCH=arm64 "${real_bash}" \
  "${repo_root}/.github/scripts/verify-fork-release-bundle.sh" \
  aarch64-apple-darwin \
  "${output_dir}/codex-aarch64-apple-darwin-bundle.tar.zst" \
  "${expected_binary_version}"
FAKE_FILE_ARCH=x86_64 "${real_bash}" \
  "${repo_root}/.github/scripts/verify-fork-release-bundle.sh" \
  x86_64-unknown-linux-musl \
  "${output_dir}/codex-x86_64-unknown-linux-musl-bundle.tar.zst" \
  "${expected_binary_version}"

mkdir -p "${fixture_root}/mac-mismatch"
cp "${fixture_root}/version-helper" "${fixture_root}/mac-mismatch/codex"
cp "${fixture_root}/version-helper-mismatch" \
  "${fixture_root}/mac-mismatch/codex-code-mode-host"
chmod +x "${fixture_root}/mac-mismatch/codex" "${fixture_root}/mac-mismatch/codex-code-mode-host"
mismatch_bundle="${output_dir}/codex-aarch64-apple-darwin-mismatch.tar.zst"
tar --use-compress-program=zstd -cf "${mismatch_bundle}" \
  -C "${fixture_root}/mac-mismatch" codex codex-code-mode-host
mismatch_output=""
if mismatch_output="$(FAKE_FILE_ARCH=arm64 "${real_bash}" \
  "${repo_root}/.github/scripts/verify-fork-release-bundle.sh" \
  aarch64-apple-darwin "${mismatch_bundle}" "${expected_binary_version}" 2>&1)"; then
  echo "bundle verifier accepted a mismatched companion version" >&2
  exit 1
fi
if [[ "${mismatch_output}" != *"codex-code-mode-host"* ]]; then
  echo "bundle verifier did not identify the mismatched companion version: ${mismatch_output}" >&2
  exit 1
fi

publish_success_output=""
if ! publish_success_output="$(${release_script} publish --publish --date "${test_date}" --output-dir "${output_dir}" 2>&1)"; then
  echo "publish failed on the verified fixture path: ${publish_success_output}" >&2
  exit 1
fi
expected_remote_sha="$(${real_git} -C "${source_repo}" rev-parse refs/remotes/origin/main)"
grep -Fq -- "--force-with-lease=refs/heads/main:${expected_remote_sha}" "${fake_git_log}" || {
  echo "publish did not force-update origin/main" >&2
  exit 1
}
grep -Fq "refs/tags/${expected_tag}:refs/tags/${expected_tag}" "${fake_git_log}" || {
  echo "publish did not push the annotated release tag" >&2
  exit 1
}
grep -Fq "release create ${expected_tag}" "${fake_gh_log}" || {
  echo "publish did not create the GitHub release" >&2
  exit 1
}
grep -Fq "build --platform linux/amd64" "${fake_docker_log}" || {
  echo "publish did not verify the Linux builder image" >&2
  exit 1
}
grep -Fq "run --rm --platform linux/amd64" "${fake_docker_log}" || {
  echo "publish did not invoke the Linux verifier" >&2
  exit 1
}
grep -Fq "aarch64-apple-darwin" "${fake_verify_log}" || {
  echo "publish did not invoke the macOS verifier" >&2
  exit 1
}

"${real_git}" -C "${source_repo}" push --quiet origin \
  "refs/tags/${expected_tag}:refs/tags/${expected_tag}"
: >"${fake_git_log}"
existing_release_output=""
if existing_release_output="$(FAKE_GH_API_MODE=existing "${release_script}" publish --publish --date "${test_date}" --output-dir "${output_dir}" 2>&1)"; then
  echo "publish accepted an existing GitHub release" >&2
  exit 1
fi
if [[ "${existing_release_output}" != *"already exists"* ]] \
  || grep -Fq -- "--force-with-lease=refs/heads/main:" "${fake_git_log}"; then
  echo "publish did not fail closed for an existing tag/release: ${existing_release_output}" >&2
  exit 1
fi

"${real_git}" -C "${source_repo}" commit --quiet --allow-empty -m conflict
conflict_date="20000103"
conflict_tag="$(${release_script} metadata --date "${conflict_date}")"
"${real_git}" -C "${source_repo}" tag -a "${conflict_tag}" -m "${conflict_tag}" HEAD^
: >"${fake_git_log}"
: >"${fake_gh_log}"
conflict_output=""
if conflict_output="$(${release_script} publish --publish --date "${conflict_date}" --output-dir "${output_dir}" 2>&1)"; then
  echo "publish accepted a conflicting local release tag" >&2
  exit 1
fi
if [[ "${conflict_output}" != *"Local tag ${conflict_tag} points"* ]] \
  || grep -Fq -- "--force-with-lease=refs/heads/main:" "${fake_git_log}" \
  || grep -Fq "refs/tags/" "${fake_git_log}" \
  || grep -Fq "release create" "${fake_gh_log}"; then
  echo "conflicting local tag did not fail before publication: ${conflict_output}" >&2
  exit 1
fi

lease_date="20000104"
lease_tag="$(${release_script} metadata --date "${lease_date}")"
: >"${fake_git_log}"
: >"${fake_gh_log}"
lease_output=""
if lease_output="$(FAKE_REJECT_MAIN_LEASE=true "${release_script}" publish --publish \
  --date "${lease_date}" --output-dir "${output_dir}" 2>&1)"; then
  echo "publish accepted a rejected force-with-lease" >&2
  exit 1
fi
if [[ "${lease_output}" != *"simulated force-with-lease rejection"* ]] \
  || ! grep -Fq -- "--force-with-lease=refs/heads/main:${expected_remote_sha}" "${fake_git_log}" \
  || grep -Fq "refs/tags/" "${fake_git_log}" \
  || grep -Fq "release create" "${fake_gh_log}" \
  || "${real_git}" -C "${source_repo}" rev-parse --verify "refs/tags/${lease_tag}" >/dev/null 2>&1; then
  echo "lease rejection did not stop later publication steps: ${lease_output}" >&2
  exit 1
fi

non_404_output=""
if non_404_output="$(FAKE_GH_API_MODE=error "${release_script}" publish --publish --date "${test_date}" --output-dir "${output_dir}" 2>&1)"; then
  echo "publish accepted a non-404 GitHub lookup failure" >&2
  exit 1
fi
if [[ "${non_404_output}" != *"Unable to confirm"* ]]; then
  echo "publish did not identify a non-404 GitHub lookup failure: ${non_404_output}" >&2
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
