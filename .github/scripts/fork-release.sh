#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  fork-release.sh metadata [--date YYYYMMDD]
  fork-release.sh build [--date YYYYMMDD] [--output-dir DIR]
  fork-release.sh verify [--date YYYYMMDD] [--output-dir DIR]
  fork-release.sh publish --publish [--date YYYYMMDD] [--output-dir DIR]
  fork-release.sh roll [--run] [--publish] [--date YYYYMMDD] [--output-dir DIR]

`publish` deliberately requires the literal `--publish` flag. It verifies the
existing artifacts, publishes the fork branch with force-with-lease, creates
and pushes one annotated tag, and creates one GitHub prerelease with the two
platform bundles.

`roll` is the canonical end-to-end path and is a dry run unless `--run` is
present. `roll --run` builds and verifies both bundles on an authorized Apple
Silicon macOS host and creates the local tag. Adding `--publish` also publishes
the fork branch, tag, and GitHub prerelease.
EOF
}

die() {
  echo "$*" >&2
  exit 1
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
codex_root="${repo_root}/codex-rs"
macos_target="aarch64-apple-darwin"
linux_target="x86_64-unknown-linux-musl"
linux_image="codex-fork-release-linux:rust-1.95.0-zig-0.14.0"
command="${1:-}"
if [[ -z "${command}" ]]; then
  usage >&2
  exit 2
fi
shift

release_date="$(date -u +%Y%m%d)"
output_dir=""
publish_confirmed="false"
run_confirmed="false"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --date)
      release_date="${2:?--date requires a value}"
      shift 2
      ;;
    --output-dir)
      output_dir="${2:?--output-dir requires a value}"
      shift 2
      ;;
    --publish)
      publish_confirmed="true"
      shift
      ;;
    --run)
      run_confirmed="true"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unexpected argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ ! "${release_date}" =~ ^[0-9]{8}$ ]]; then
  echo "Release date must use YYYYMMDD: ${release_date}" >&2
  exit 2
fi

base_version="$(tr -d '[:space:]' < "${codex_root}/fork-version.txt")"
short_sha="$(git -C "${repo_root}" rev-parse --short=9 HEAD)"
revision="$(git -C "${repo_root}" rev-parse --short=12 HEAD)"
head_sha="$(git -C "${repo_root}" rev-parse HEAD)"
release_tag="rust-v${base_version}-fork.${release_date}.g${short_sha}"
expected_version="codex-cli ${base_version}+fork.${revision}"
if [[ -z "${output_dir}" ]]; then
  output_dir="${codex_root}/dist/fork-release/${release_tag}"
fi
if [[ "${output_dir}" != /* ]]; then
  output_dir="${repo_root}/${output_dir}"
fi
macos_bundle="${output_dir}/codex-${macos_target}-bundle.tar.zst"
linux_bundle="${output_dir}/codex-${linux_target}-bundle.tar.zst"

require_clean_head() {
  if ! git -C "${repo_root}" diff --quiet || ! git -C "${repo_root}" diff --cached --quiet; then
    die "Refusing to release a dirty tracked working tree"
  fi
}

require_fork_release_head() {
  local subject
  subject="$(git -C "${repo_root}" log -1 --format=%s HEAD)"
  [[ "${subject}" == "ci(release): [fork] build prerelease bundle" ]] ||
    die "HEAD must be the canonical fork release commit"
}

require_fork_branch() {
  local branch
  branch="$(git -C "${repo_root}" symbolic-ref --quiet --short HEAD 2>/dev/null || true)"
  [[ "${branch}" == "fork" ]] || die "Release publication requires the local fork branch"
}

require_macos_builder() {
  [[ "$(uname -s)" == Darwin ]] || die "The local fork release builder requires macOS"
  [[ "$(uname -m)" == arm64 ]] || die "The local fork release builder requires Apple arm64"
  command -v docker >/dev/null 2>&1 || die "docker is required for the Linux x64 build"
  docker info >/dev/null 2>&1 || die "The OrbStack Docker engine is not reachable"
}

ensure_linux_image() {
  docker build \
    --platform linux/amd64 \
    --tag "${linux_image}" \
    --file "${repo_root}/.github/docker/fork-release-linux.Dockerfile" \
    "${repo_root}/.github/docker"
}

verify_macos_bundle() {
  bash "${repo_root}/.github/scripts/verify-fork-release-bundle.sh" \
    "${macos_target}" "${macos_bundle}" "${expected_version}"
}

verify_linux_bundle() {
  ensure_linux_image
  docker run --rm --platform linux/amd64 \
    --mount "type=bind,src=${repo_root},dst=/workspace,readonly" \
    --mount "type=bind,src=${output_dir},dst=/output,readonly" \
    "${linux_image}" \
    bash /workspace/.github/scripts/verify-fork-release-bundle.sh \
      "${linux_target}" \
      "/output/$(basename "${linux_bundle}")" \
      "${expected_version}"
}

build_bundles() {
  require_clean_head
  require_macos_builder
  local release_cache="${codex_root}/target/fork-release-cache"
  mkdir -p "${output_dir}" "${release_cache}/macos" "${release_cache}/linux"

  echo "Building ${release_tag} for ${macos_target}"
  GITHUB_WORKSPACE="${repo_root}" \
  RUNNER_TEMP="${release_cache}/macos" \
  CARGO_TARGET_DIR="${codex_root}/target/fork-release" \
    bash "${repo_root}/.github/scripts/build-fork-release-target.sh" \
      "${macos_target}" "${output_dir}"

  ensure_linux_image
  echo "Building ${release_tag} for ${linux_target} in OrbStack"
  docker run --rm --platform linux/amd64 \
    --mount "type=bind,src=${repo_root},dst=/workspace,readonly" \
    --mount "type=bind,src=${output_dir},dst=/output" \
    --mount "type=bind,src=${release_cache}/linux,dst=/release-tmp" \
    --mount type=volume,src=codex-fork-release-cargo,dst=/cargo \
    --mount type=volume,src=codex-fork-release-target,dst=/target \
    --env CARGO_HOME=/cargo \
    --env CARGO_TARGET_DIR=/target \
    --env GITHUB_WORKSPACE=/workspace \
    --env RUNNER_TEMP=/release-tmp \
    "${linux_image}" \
    bash -lc '
      set -euo pipefail
      git config --global --add safe.directory /workspace
      bash /workspace/.github/scripts/build-fork-release-target.sh \
        x86_64-unknown-linux-musl /output
    '

  verify_macos_bundle
  verify_linux_bundle
  echo "Built and verified both bundles in ${output_dir}"
}

verify_bundles() {
  mkdir -p "${output_dir}"
  verify_macos_bundle
  require_macos_builder
  verify_linux_bundle
}

local_tag_sha() {
  git -C "${repo_root}" rev-list -n 1 "${release_tag}" 2>/dev/null || true
}

remote_tag_sha() {
  git -C "${repo_root}" ls-remote origin \
    "refs/tags/${release_tag}" "refs/tags/${release_tag}^{}" |
    awk '
      NR == 1 { tag_sha = $1 }
      $2 ~ /\^\{\}$/ { peeled_sha = $1 }
      END {
        if (peeled_sha != "") print peeled_sha
        else print tag_sha
      }
    '
}

ensure_local_tag() {
  local tag_sha
  tag_sha="$(local_tag_sha)"
  if [[ -n "${tag_sha}" && "${tag_sha}" != "${head_sha}" ]]; then
    die "Local tag ${release_tag} points to ${tag_sha}, expected ${head_sha}"
  fi
  if [[ -z "${tag_sha}" ]]; then
    git -C "${repo_root}" tag -a "${release_tag}" -m "${release_tag}" "${head_sha}"
  fi
}

publish_fork_branch() {
  local expected_remote_sha
  expected_remote_sha="$(
    git -C "${repo_root}" rev-parse --verify refs/remotes/origin/fork 2>/dev/null || true
  )"
  [[ -n "${expected_remote_sha}" ]] ||
    die "origin/fork is not available locally; fetch it before publishing"
  git -C "${repo_root}" push \
    --force-with-lease="refs/heads/fork:${expected_remote_sha}" \
    origin HEAD:refs/heads/fork
}

publish_verified_release() {
  require_fork_release_head
  require_fork_branch
  command -v gh >/dev/null 2>&1 || die "gh is required to publish the prerelease"
  gh auth status --hostname github.com >/dev/null 2>&1 ||
    die "gh must be authenticated with github.com before publishing"

  local remote_sha
  remote_sha="$(remote_tag_sha)"
  if [[ -n "${remote_sha}" && "${remote_sha}" != "${head_sha}" ]]; then
    die "Remote tag ${release_tag} points to ${remote_sha}, expected ${head_sha}"
  fi
  local release_lookup
  if release_lookup="$(
    gh api "repos/alleneubank/codex/releases/tags/${release_tag}" 2>&1
  )"; then
    die "GitHub release ${release_tag} already exists; refusing to replace its assets"
  fi
  if [[ "${release_lookup}" != *"(HTTP 404)"* ]]; then
    die "Unable to confirm that GitHub release ${release_tag} is absent: ${release_lookup}"
  fi

  publish_fork_branch
  ensure_local_tag
  if [[ -z "${remote_sha}" ]]; then
    git -C "${repo_root}" push origin "refs/tags/${release_tag}:refs/tags/${release_tag}"
  fi

  gh release create "${release_tag}" \
    --repo alleneubank/codex \
    --verify-tag \
    --prerelease \
    --title "${release_tag}" \
    --notes "Fork prerelease from commit ${head_sha}. Linux x64 musl includes codex, codex-code-mode-host, and codex-resources/bwrap; macOS arm64 includes codex and codex-code-mode-host." \
    "${linux_bundle}" \
    "${macos_bundle}"
}

publish_release() {
  if [[ "${publish_confirmed}" != true ]]; then
    echo "publish requires the literal --publish flag" >&2
    exit 2
  fi
  require_clean_head
  verify_bundles
  publish_verified_release
}

roll_release() {
  require_fork_release_head
  if [[ "${publish_confirmed}" == true && "${run_confirmed}" != true ]]; then
    die "roll --publish requires the literal --run flag"
  fi
  if [[ "${run_confirmed}" != true ]]; then
    cat <<EOF
Fork release dry run
  candidate tag: ${release_tag}
  source commit: ${head_sha}
  output directory: ${output_dir}
  required builder: authorized Apple Silicon macOS host with OrbStack Docker

No branch, tag, release, or artifact was changed.
Run with --run to build and verify locally; add --publish to publish the fork
branch, annotated tag, and GitHub prerelease.
EOF
    return
  fi

  require_fork_branch
  build_bundles
  ensure_local_tag
  if [[ "${publish_confirmed}" == true ]]; then
    publish_verified_release
  else
    echo "Prepared local release ${release_tag}; rerun with --run --publish to publish it"
  fi
}

case "${command}" in
  metadata)
    [[ "${publish_confirmed}" == false ]] || die "metadata does not accept --publish"
    [[ "${run_confirmed}" == false ]] || die "metadata does not accept --run"
    printf '%s\n' "${release_tag}"
    ;;
  build)
    [[ "${publish_confirmed}" == false ]] || die "build does not accept --publish"
    [[ "${run_confirmed}" == false ]] || die "build does not accept --run"
    build_bundles
    ;;
  verify)
    [[ "${publish_confirmed}" == false ]] || die "verify does not accept --publish"
    [[ "${run_confirmed}" == false ]] || die "verify does not accept --run"
    verify_bundles
    ;;
  publish)
    [[ "${run_confirmed}" == false ]] || die "publish does not accept --run"
    publish_release
    ;;
  roll)
    roll_release
    ;;
  -h|--help|help)
    usage
    ;;
  *)
    echo "Unknown command: ${command}" >&2
    usage >&2
    exit 2
    ;;
esac
