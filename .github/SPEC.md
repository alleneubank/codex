# Fork Release Versioning

The fork publishes immutable release bundles from upstream development snapshots, but upstream
keeps workspace crates at `0.0.0` and publishes stable product versions on separate
`rust-vX.Y.Z` tags. A checked-in fork version therefore preserves reproducible builds, while a
sync-time resolver prevents that pin from silently falling behind the latest stable release.

## Domain Model

- An **upstream stable tag** matches exactly `rust-vX.Y.Z`, where each component is numeric.
- The **latest stable version** is the numerically greatest upstream stable tag. Prerelease,
  malformed, and fork tags are outside that set.
- The **fork version pin** is the stable SemVer stored in `codex-rs/fork-version.txt`.
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

## Invariants

- A successful fork release cannot embed a stale upstream-comparable stable version.
- Rebuilding one source commit remains deterministic because builds consume the committed pin.
- Failure to read upstream release state is an error, never permission to reuse the existing pin.
- Release candidates never select an upstream alpha, beta, release candidate, or fork tag.

## Non-goals

- Deriving a product version from upstream `main` ancestry or its `0.0.0` workspace version.
- Performing network lookups inside Cargo build scripts or target-specific bundle builders.
- Updating dotfiles or deploying the release to hosts.

## Risk

- **High: release infrastructure.** A false pass publishes mislabeled immutable binaries; a false
  failure blocks a release and must remain actionable.

## Acceptance Criteria

- [ ] A deterministic fake upstream proves stable numeric selection and prerelease exclusion.
- [ ] Stale, malformed, missing, and unreachable upstream cases fail closed with actionable errors.
- [ ] The update command writes the resolved version and the subsequent check passes.
- [ ] The fork release contract rejects a stale real pin before computing release metadata.
- [ ] ShellCheck, formatting, release dry-run, and both bundle verifiers pass.
- [ ] The published prerelease is independently verified by GitHub Actions.

## Test Traceability

- `REQ-FORK-VERSION-001` through `REQ-FORK-VERSION-004`:
  `.github/scripts/test-fork-version.sh`
- `REQ-FORK-VERSION-005` and `REQ-FORK-VERSION-006`:
  `.github/scripts/test-fork-release.sh` and `.github/workflows/rust-release.yml`
