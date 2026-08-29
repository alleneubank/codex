#!/usr/bin/env bash
# Sourced by fork release scripts that run Python needing tomllib (3.11+).
# Bare `python3` is not enough on a local builder: macOS ships 3.9 at
# /usr/bin/python3 and a PATH that lists system bins first resolves there even
# with a newer interpreter installed. Probe candidates by capability.

fork_python_bin() {
  local candidate
  for candidate in \
    "${FORK_PYTHON:-}" \
    python3.14 python3.13 python3.12 python3.11 \
    /opt/homebrew/bin/python3 /usr/local/bin/python3 \
    python3
  do
    [[ -n "$candidate" ]] || continue
    command -v "$candidate" >/dev/null 2>&1 || continue
    "$candidate" -c 'import sys, tomllib; sys.exit(0)' >/dev/null 2>&1 || continue
    printf '%s\n' "$candidate"
    return 0
  done
  if command -v uv >/dev/null 2>&1; then
    candidate="$(uv python find --no-python-downloads '>=3.11' 2>/dev/null || true)"
    if [[ -n "$candidate" && -x "$candidate" ]] \
      && "$candidate" -c 'import sys, tomllib; sys.exit(0)' >/dev/null 2>&1; then
      printf '%s\n' "$candidate"
      return 0
    fi
  fi
  echo "no python with tomllib (3.11+) found; set FORK_PYTHON" >&2
  return 1
}
