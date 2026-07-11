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
- The current user authorization covers rewriting and publishing the Codex fork plus one new
  immutable prerelease; it does not cover dotfiles or host deployment.

## Boundary

Git force-push, tag creation, GitHub release publication, live credentials, and any replacement of
an already published artifact require the human. This run is authorized to force-with-lease update
`origin/fork` and publish the new version-correct prerelease after all floors pass; exact refs and
assets are restated immediately before publication.
