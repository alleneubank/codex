#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
setup_script="${repo_root}/.github/scripts/setup-rusty-v8.sh"
test_root="$(mktemp -d)"
trap 'rm -rf "${test_root}"' EXIT

fail() {
  echo "$*" >&2
  exit 1
}

hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

file_mtimes() {
  if [[ "$(uname -s)" == Darwin ]]; then
    stat -f '%m' "$@"
  else
    stat -c '%Y' "$@"
  fi
}

target="test-target"
version="$(python3 "${repo_root}/.github/scripts/rusty_v8_bazel.py" resolved-v8-crate-version)"
release_tag="rusty-v8-v${version}"
archive_name="librusty_v8_release_${target}.a.gz"
binding_name="src_binding_release_${target}.rs"
checksums_name="rusty_v8_release_${target}.sha256"
fixture_dir="${test_root}/fixtures"
fake_bin="${test_root}/bin"
curl_log="${test_root}/curl.log"
runner_temp="${test_root}/runner-temp"
mkdir -p "${fixture_dir}" "${fake_bin}" "${runner_temp}"

printf 'archive fixture\n' >"${fixture_dir}/${archive_name}"
printf 'binding fixture\n' >"${fixture_dir}/${binding_name}"
printf '%s  %s\n%s  %s\n' \
  "$(hash_file "${fixture_dir}/${archive_name}")" \
  "${archive_name}" \
  "$(hash_file "${fixture_dir}/${binding_name}")" \
  "${binding_name}" >"${fixture_dir}/${checksums_name}"

cat >"${fake_bin}/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

output=""
url=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    -o)
      output="${2:?curl -o requires a path}"
      shift 2
      ;;
    -*)
      shift
      ;;
    *)
      url="$1"
      shift
      ;;
  esac
done
[[ -n "${output}" && -n "${url}" ]]
name="${url##*/}"
cp "${FAKE_CURL_SOURCE}/${name}" "${output}"
printf '%s\n' "${name}" >>"${FAKE_CURL_LOG}"
EOF
chmod +x "${fake_bin}/curl"
export FAKE_CURL_SOURCE="${fixture_dir}"
export FAKE_CURL_LOG="${curl_log}"

run_setup() {
  local github_env="$1"
  : >"${github_env}"
  PATH="${fake_bin}:${PATH}" \
    GITHUB_WORKSPACE="${repo_root}" \
    GITHUB_ENV="${github_env}" \
    RUNNER_TEMP="${runner_temp}" \
    bash "${setup_script}" "${target}" >/dev/null
}

first_env="${test_root}/first.env"
run_setup "${first_env}"
binding_dir="${runner_temp}/rusty_v8/${release_tag}/${target}"
for name in "${archive_name}" "${binding_name}" "${checksums_name}"; do
  [[ -f "${binding_dir}/${name}" ]] || fail "missing version-addressed cached input: ${name}"
  touch -t 200001010000 "${binding_dir}/${name}"
done
[[ "$(wc -l <"${curl_log}" | tr -d ' ')" == "3" ]] ||
  fail "cache miss did not download exactly three inputs"

before_mtimes="$(file_mtimes "${binding_dir}"/*)"
second_env="${test_root}/second.env"
run_setup "${second_env}"
after_mtimes="$(file_mtimes "${binding_dir}"/*)"
[[ "$(wc -l <"${curl_log}" | tr -d ' ')" == "3" ]] ||
  fail "valid cache hit performed another download"
[[ "${after_mtimes}" == "${before_mtimes}" ]] || fail "valid cache hit changed cached input mtimes"
grep -Fqx "RUSTY_V8_ARCHIVE=${binding_dir}/${archive_name}" "${second_env}" ||
  fail "cache hit exported the wrong archive path"
grep -Fqx "RUSTY_V8_SRC_BINDING_PATH=${binding_dir}/${binding_name}" "${second_env}" ||
  fail "cache hit exported the wrong binding path"

printf 'corrupted archive\n' >"${binding_dir}/${archive_name}"
third_env="${test_root}/third.env"
run_setup "${third_env}"
[[ "$(wc -l <"${curl_log}" | tr -d ' ')" == "6" ]] ||
  fail "invalid cache did not refresh all three inputs"
(
  cd "${binding_dir}"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "${checksums_name}" >/dev/null
  else
    shasum -a 256 -c "${checksums_name}" >/dev/null
  fi
) || fail "refreshed cache did not pass checksum verification"

echo "rusty_v8 setup cache contract passed"
