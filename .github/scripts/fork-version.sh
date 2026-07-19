#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  fork-version.sh resolve [--upstream REPOSITORY] [--version-file FILE]
  fork-version.sh check [--upstream REPOSITORY] [--version-file FILE]
  fork-version.sh update [--upstream REPOSITORY] [--version-file FILE]

Resolve the latest stable upstream `rust-vX.Y.Z` tag. `check` compares it with
the committed fork version; `update` writes the resolved SemVer to that file.
Normal builds do not invoke this script and remain network-independent.
EOF
}

die() {
  echo "$*" >&2
  exit 1
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
command="${1:-}"
if [[ -z "${command}" ]]; then
  usage >&2
  exit 2
fi
shift

upstream="${CODEX_FORK_VERSION_UPSTREAM:-https://github.com/openai/codex.git}"
version_file="${CODEX_FORK_VERSION_FILE:-${repo_root}/codex-rs/fork-version.txt}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --upstream)
      upstream="${2:?--upstream requires a repository}"
      shift 2
      ;;
    --version-file)
      version_file="${2:?--version-file requires a file}"
      shift 2
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

stable_semver_regex='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
stable_ref_regex='^refs/tags/rust-v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'

resolve_latest_stable() {
  local refs
  if ! refs="$(
    GIT_TERMINAL_PROMPT=0 git \
      -c http.lowSpeedLimit=1 \
      -c http.lowSpeedTime=30 \
      ls-remote --refs --tags "${upstream}" 'refs/tags/rust-v*' 2>&1
  )"; then
    die "unable to query upstream tags from ${upstream}: ${refs}"
  fi

  local found="false"
  local best_major=0
  local best_minor=0
  local best_patch=0
  local ref
  while IFS=$'\t' read -r _ ref; do
    if [[ "${ref}" =~ ${stable_ref_regex} ]]; then
      local major=$((10#${BASH_REMATCH[1]}))
      local minor=$((10#${BASH_REMATCH[2]}))
      local patch=$((10#${BASH_REMATCH[3]}))
      if [[ "${found}" == "false" ]] \
        || ((major > best_major)) \
        || ((major == best_major && minor > best_minor)) \
        || ((major == best_major && minor == best_minor && patch > best_patch)); then
        found="true"
        best_major="${major}"
        best_minor="${minor}"
        best_patch="${patch}"
      fi
    fi
  done <<<"${refs}"

  [[ "${found}" == "true" ]] || die "no stable rust-vX.Y.Z tags found at ${upstream}"
  printf '%s.%s.%s\n' "${best_major}" "${best_minor}" "${best_patch}"
}

read_pinned_version() {
  [[ -f "${version_file}" ]] || die "fork version file does not exist: ${version_file}"
  local pinned
  pinned="$(tr -d '[:space:]' <"${version_file}")"
  [[ "${pinned}" =~ ${stable_semver_regex} ]] ||
    die "${version_file} must contain stable SemVer X.Y.Z, got: ${pinned:-<empty>}"
  printf '%s\n' "${pinned}"
}

case "${command}" in
  resolve)
    resolve_latest_stable
    ;;
  check)
    pinned_version="$(read_pinned_version)"
    latest_version="$(resolve_latest_stable)"
    [[ "${pinned_version}" == "${latest_version}" ]] ||
      die "pinned fork version ${pinned_version} is stale; latest stable upstream version is ${latest_version}. Run: just update-fork-version"
    printf '%s\n' "${pinned_version}"
    ;;
  update)
    latest_version="$(resolve_latest_stable)"
    printf '%s\n' "${latest_version}" >"${version_file}"
    printf 'Updated %s to %s\n' "${version_file}" "${latest_version}"
    ;;
  *)
    echo "Unknown command: ${command}" >&2
    usage >&2
    exit 2
    ;;
esac
