# Fork Maintenance and Release Versioning

The fork maintains its product on `origin/main`, rebased onto `upstream/main`. Upstream keeps
workspace crates at `0.0.0` and publishes stable product versions on separate `rust-vX.Y.Z` tags.
A checked-in fork version preserves reproducible builds, while sync-time and pre-push verification
prevent that pin from silently falling behind the latest stable release.

## Domain Model

- An **upstream stable tag** matches exactly `rust-vX.Y.Z`, where each component is numeric.
- The **latest stable version** is the numerically greatest upstream stable tag. Prerelease,
  malformed, and fork tags are outside that set.
- The **fork version pin** is the stable SemVer stored in `codex-rs/fork-version.txt`.
- The **fork product branch** is local `main` published as `origin/main`.
- A **fork release identity** combines the pin, release date, and source revision without changing
  the upstream-comparable SemVer embedded in the binaries.

## Requirements

- **REQ-FORK-VERSION-001:** The resolver queries the configured upstream Git repository and selects
  the numerically greatest tag matching exactly `rust-vX.Y.Z`.
- **REQ-FORK-VERSION-002:** The resolver ignores prerelease tags, fork tags, and malformed version
  tags.
- **REQ-FORK-VERSION-003:** The update command writes the resolved stable SemVer to
  `codex-rs/fork-version.txt`; normal builds read only that committed pin and perform no network
  lookup.
- **REQ-FORK-VERSION-004:** The check command fails when the upstream query fails, no stable tag is
  present, the pin is invalid, or the pin differs from the resolved latest stable version.
- **REQ-FORK-VERSION-005:** Every fork release command checks freshness before deriving a candidate
  tag or expected binary version.
- **REQ-FORK-VERSION-006:** Published fork tags and assets are immutable; a corrected build receives
  a new source revision and release tag.
- **REQ-FORK-VERSION-007:** A locally built and verified release can be published through a separate
  command that preserves its date and output identity while re-verifying the existing bundles
  without rebuilding them.
- **REQ-FORK-VERSION-008:** Rusty V8 inputs are cached by exact upstream release and target, reused
  only after checksum verification, and fully refreshed when the cache is missing or invalid.
- **REQ-FORK-VERSION-009:** `origin/main` is the only maintained fork product branch;
  non-deletion pushes to the retired `origin/fork` ref fail locally.
- **REQ-FORK-VERSION-010:** A local pre-push hook checks the version pin from the exact commit being
  pushed to `origin/main` and fails closed when the pin is stale or unavailable.

## Invariants

- A successful `origin/main` push or fork release cannot carry a stale upstream-comparable stable
  version when the local hook is installed.
- Rebuilding one source commit remains deterministic because builds consume the committed pin.
- Failure to read upstream release state is an error, never permission to reuse the existing pin.
- Release candidates never select an upstream alpha, beta, release candidate, or fork tag.
- Fork development and releases do not depend on a second long-lived product branch.

## Non-goals

- Deriving a product version from upstream `main` ancestry or its `0.0.0` workspace version.
- Performing network lookups inside Cargo build scripts or target-specific bundle builders.
- Adding fork-specific GitHub Actions checks.
- Updating dotfiles or deploying a release to hosts.

## Risk

- **High: release infrastructure.** A false pass publishes mislabeled immutable binaries; a false
  failure blocks a release and must remain actionable.

## Acceptance Criteria

- [ ] A deterministic fake upstream proves stable numeric selection and prerelease exclusion.
- [ ] Stale, malformed, missing, and unreachable upstream cases fail closed with actionable errors.
- [ ] The update command writes the resolved version and the subsequent check passes.
- [ ] The pre-push harness verifies the exact pushed commit and enforces the main-only topology.
- [ ] The fork release contract rejects a stale real pin before computing release metadata.
- [ ] The staged publication path re-verifies existing artifacts without rebuilding them.
- [ ] Repeated Rusty V8 setup preserves verified cache files and refreshes corrupted inputs.
- [ ] ShellCheck, formatting, release dry-run, and both bundle verifiers pass locally.

## Test Traceability

- `REQ-FORK-VERSION-001` through `REQ-FORK-VERSION-004`:
  `.github/scripts/test-fork-version.sh`
- `REQ-FORK-VERSION-005` through `REQ-FORK-VERSION-007`:
  `.github/scripts/test-fork-release.sh`
- `REQ-FORK-VERSION-008`: `.github/scripts/test-setup-rusty-v8.sh`
- `REQ-FORK-VERSION-009` and `REQ-FORK-VERSION-010`:
  `.github/scripts/test-fork-pre-push.sh`
