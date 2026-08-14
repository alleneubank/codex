#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: verify-fork-release-bundle.sh <rust-target> <bundle> <expected-version>" >&2
}

if [[ $# -ne 3 ]]; then
  usage
  exit 2
fi
target="$1"
bundle="$2"
expected_version="$3"
if [[ ! -f "${bundle}" ]]; then
  echo "Missing release bundle: ${bundle}" >&2
  exit 1
fi

verify_root="$(mktemp -d "${TMPDIR:-/tmp}/codex-fork-bundle.XXXXXX")"
trap 'rm -rf "${verify_root}"' EXIT
zstd -d -c "${bundle}" | tar -xf - -C "${verify_root}"

expected_entries=(codex codex-code-mode-host)
case "${target}" in
  aarch64-apple-darwin)
    architecture_regex="arm64"
    ;;
  x86_64-unknown-linux-musl)
    architecture_regex="x86[-_]64|x86-64"
    expected_entries+=(codex-resources/bwrap)
    ;;
  *)
    echo "Unsupported fork release target: ${target}" >&2
    exit 2
    ;;
esac

for entry in "${expected_entries[@]}"; do
  if [[ ! -x "${verify_root}/${entry}" ]]; then
    echo "Bundle entry is missing or not executable: ${entry}" >&2
    exit 1
  fi
  if ! file "${verify_root}/${entry}" | grep -Eq "${architecture_regex}"; then
    echo "Bundle entry has the wrong architecture: ${entry}" >&2
    file "${verify_root}/${entry}" >&2
    exit 1
  fi
done

actual_entries="$(find "${verify_root}" -type f -print | sed "s#^${verify_root}/##" | sort)"
expected_listing="$(printf '%s\n' "${expected_entries[@]}" | sort)"
if [[ "${actual_entries}" != "${expected_listing}" ]]; then
  echo "Bundle contains unexpected entries" >&2
  diff -u <(printf '%s\n' "${expected_listing}") <(printf '%s\n' "${actual_entries}") >&2 || true
  exit 1
fi

actual_version="$("${verify_root}/codex" --version)"
if [[ "${actual_version}" != "${expected_version}" ]]; then
  echo "Expected ${expected_version}, got ${actual_version}" >&2
  exit 1
fi

actual_code_mode_host_version="$("${verify_root}/codex-code-mode-host" --version)"
if [[ "${actual_code_mode_host_version}" != "${expected_version}" ]]; then
  echo "Expected ${expected_version} from codex-code-mode-host, got ${actual_code_mode_host_version}" >&2
  exit 1
fi

if [[ "${target}" == x86_64-unknown-linux-musl ]]; then
  actual_bwrap_version="$("${verify_root}/codex-resources/bwrap" --version)"
  expected_bwrap_version="bubblewrap built for Codex ${expected_version}"
  if [[ "${actual_bwrap_version}" != "${expected_bwrap_version}" ]]; then
    echo "Expected ${expected_bwrap_version} from bwrap, got ${actual_bwrap_version}" >&2
    exit 1
  fi
fi

echo "Verified ${bundle} (${target}, ${expected_version})"
