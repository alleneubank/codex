# Fork Release Quality Law

> Law doc for the fork release surface, present-tense, no narrated history. Amend Decisions and
> Boundary only with human confirmation; git is the changelog.

## Bar

A fork release is shippable only when its committed stable version matches upstream, both bundles
verify from the exact source commit, and an independent post-publication workflow passes.

## Dimensions

- Version correctness
- Reproducibility
- Fail-closed safety
- Cross-platform portability
- Publication auditability

## Floors

- A fake Git upstream exercises numeric stable selection, exclusions, stale-pin rejection, update,
  missing-tag failure, and remote-query failure through `.github/scripts/test-fork-version.sh`.
- `.github/scripts/test-fork-release.sh` proves the release entry point enforces the freshness gate
  and immutable candidate shape.
- `.github/scripts/test-setup-rusty-v8.sh` proves repeat setup preserves verified V8 cache inputs
  and corrupt inputs trigger a complete checked refresh.
- ShellCheck, `just fmt`, the release dry-run, and both real bundle verifiers pass from a clean tree.
- GitHub Actions downloads and verifies both published assets from the exact annotated tag.

## Oracle

The deterministic fake remote owns version-selection checks independently of GitHub state. The
published-release workflow is the final independent oracle because it checks the immutable remote
tag and downloads instead of trusting the publisher's local artifacts.

## Never

- Never accept a stale pin because upstream is unavailable.
- Never treat a prerelease or fork tag as the upstream stable version.
- Never perform version discovery from a Cargo build script.
- Never move a published tag or replace its assets.
- Never publish an artifact whose source SHA differs from its embedded fork revision.

## Decisions

- Latest stable means the greatest globally published `rust-vX.Y.Z` tag; patch releases may live on
  an upstream release branch rather than `main`.
- Resolution occurs during sync and release preflight, then the result is committed for reproducible
  builds.
- Fork-only release automation remains in the single top
  `ci(release): [fork] build prerelease bundle` commit.
- A completed `roll --run` hands publication to `publish --publish`; rebuilding is reserved for a
  one-step roll whose publication was authorized before the build began. The handoff preserves the
  prepared date and output path so UTC rollover or a custom directory cannot select other assets.
- Rusty V8 cache entries are scoped by exact release tag and target, and checksum validity—not
  presence alone—decides whether an entry is reusable.
- The current user authorization covers force-with-lease updating `origin/fork` with this release
  optimization. It does not cover moving or replacing the published prerelease or its assets.

## Boundary

Git force-push, tag creation, GitHub release publication, live credentials, and any replacement of
an already published artifact require the human. This run is authorized to force-with-lease update
`origin/fork` after all local floors pass; the existing `rust-v0.144.4-fork.20260715.g36d685baf`
tag and release remain immutable.
