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
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=.github/scripts/fork-python.sh
source "${repo_root}/.github/scripts/fork-python.sh"
python_bin="$(fork_python_bin)"
if [[ ! -f "${bundle}" ]]; then
  echo "Missing release bundle: ${bundle}" >&2
  exit 1
fi

verify_root="$(mktemp -d "${TMPDIR:-/tmp}/codex-fork-bundle.XXXXXX")"
daemon_home=""
daemon_running=false
cleanup() {
  if [[ "${daemon_running}" == true ]]; then
    CODEX_HOME="${daemon_home}" "${verify_root}/bin/codex" app-server daemon stop >&2 || return
  fi
  rm -rf "${verify_root}"
  if [[ -n "${daemon_home}" ]]; then
    rm -rf "${daemon_home}"
  fi
}
trap cleanup EXIT
zstd -d -c "${bundle}" | tar -xf - -C "${verify_root}"

expected_entries=(bin/codex bin/codex-code-mode-host codex-path/rg codex-resources/zsh/bin/zsh)
case "${target}" in
  aarch64-apple-darwin)
    executable_regex="Mach-O 64-bit.*arm64"
    expected_format="Mach-O arm64 executable"
    ;;
  x86_64-unknown-linux-musl)
    # The musl bundle must run without a host ELF interpreter or shared libraries.
    executable_regex="ELF 64-bit LSB (pie )?executable, x86-64.*(statically linked|static-pie linked)"
    expected_format="static Linux x86_64 executable"
    expected_entries+=(codex-resources/bwrap)
    ;;
  *)
    echo "Unsupported fork release target: ${target}" >&2
    exit 2
    ;;
esac

CODEX_REPO_ROOT="${repo_root}" "${python_bin}" - "${repo_root}/scripts" "${verify_root}" "${target}" "${expected_version#codex-cli }" <<'PY'
import json
import sys
from pathlib import Path

sys.path.insert(0, sys.argv[1])
from codex_package.layout import validate_package_dir
from codex_package.targets import PACKAGE_VARIANTS, TARGET_SPECS

root = Path(sys.argv[2])
validate_package_dir(root, PACKAGE_VARIANTS["codex"], TARGET_SPECS[sys.argv[3]], include_zsh=True)
metadata = json.loads((root / "codex-package.json").read_text())
if metadata["version"] != sys.argv[4]:
    raise SystemExit(f"Unexpected package version: {metadata['version']}")
PY

for entry in "${expected_entries[@]}"; do
  if [[ ! -x "${verify_root}/${entry}" ]]; then
    echo "Bundle entry is missing or not executable: ${entry}" >&2
    exit 1
  fi
  executable_description="$(file "${verify_root}/${entry}")"
  entry_regex="${executable_regex}"
  entry_format="${expected_format}"
  if [[ "${target}" == x86_64-unknown-linux-musl && "${entry}" == codex-resources/zsh/bin/zsh ]]; then
    # Upstream supplies the optional patched zsh as a GNU/Linux resource.
    entry_regex="ELF 64-bit LSB (pie )?executable, x86-64"
    entry_format="Linux x86_64 executable"
  fi
  if ! grep -Eq "${entry_regex}" <<<"${executable_description}"; then
    echo "Bundle entry is not a ${entry_format}: ${entry}" >&2
    echo "${executable_description}" >&2
    exit 1
  fi
done

actual_entries="$(find "${verify_root}" -type f -print | sed "s#^${verify_root}/##" | sort)"
expected_listing="$(printf '%s\n' "${expected_entries[@]}" codex-package.json | sort)"
if [[ "${actual_entries}" != "${expected_listing}" ]]; then
  echo "Bundle contains unexpected entries" >&2
  diff -u <(printf '%s\n' "${expected_listing}") <(printf '%s\n' "${actual_entries}") >&2 || true
  exit 1
fi

actual_version="$("${verify_root}/bin/codex" --version)"
if [[ "${actual_version}" != "${expected_version}" ]]; then
  echo "Expected ${expected_version}, got ${actual_version}" >&2
  exit 1
fi

actual_code_mode_host_version="$("${verify_root}/bin/codex-code-mode-host" --version)"
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

# An empty home exercises the same package installation required by plain `codex`.
daemon_home="${verify_root}-home"
mkdir -p "${daemon_home}"
daemon_running=true
daemon_output="$(CODEX_HOME="${daemon_home}" "${verify_root}/bin/codex" app-server daemon start)"
if [[ "${daemon_output}" != *'"status":"started"'* ]]; then
  echo "Packaged daemon did not start: ${daemon_output}" >&2
  exit 1
fi
daemon_output="$(CODEX_HOME="${daemon_home}" "${verify_root}/bin/codex" app-server daemon stop)"
if [[ "${daemon_output}" != *'"status":"stopped"'* ]]; then
  echo "Packaged daemon did not stop: ${daemon_output}" >&2
  exit 1
fi
daemon_running=false

echo "Verified ${bundle} (${target}, ${expected_version})"
