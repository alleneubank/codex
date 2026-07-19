# Fork Maintenance Quality Law

> Law doc for the fork maintenance surface, present-tense, no narrated history. Amend Decisions and
> Boundary only with human confirmation; git is the changelog.

## Bar

The fork is shippable only when `origin/main` is based on the intended upstream revision, its
committed stable version matches upstream, both release bundles verify from the exact source
commit, and the local maintenance harness passes.

## Dimensions

- Version correctness
- Reproducibility
- Fail-closed safety
- Branch-topology clarity
- Cross-platform portability
- Publication auditability

## Floors

- A fake Git upstream exercises numeric stable selection, exclusions, stale-pin rejection, update,
  missing-tag failure, and remote-query failure through `.github/scripts/test-fork-version.sh`.
- `.github/scripts/test-fork-pre-push.sh` proves that the hook checks the exact pushed commit,
  protects `origin/main`, and rejects recreation of `origin/fork`.
- `.github/scripts/test-fork-release.sh` proves the release entry point enforces the freshness gate,
  main-only source policy, staged publication, and immutable candidate shape.
- `.github/scripts/test-setup-rusty-v8.sh` proves repeat setup preserves verified V8 cache inputs
  and corrupt inputs trigger a complete checked refresh.
- ShellCheck, `just fmt`, the release dry-run, targeted project tests, and both real bundle verifiers
  pass locally before publication.

## Oracle

The deterministic fake remotes own version-selection and branch-policy checks independently of live
GitHub state. Bundle verification inspects the archived executables and their embedded version
instead of trusting the publisher's build log. Live publication remains a human-attended final gate.

## Never

- Never accept a stale pin because upstream is unavailable.
- Never treat a prerelease or fork tag as the upstream stable version.
- Never perform version discovery from a Cargo build script.
- Never recreate `origin/fork` as a maintained product branch.
- Never move a published tag or replace its assets.
- Never publish an artifact whose source SHA differs from its embedded fork revision.

## Decisions

- `origin/main` is the sole maintained fork product and release-source branch;
  `upstream/main` is its rebase source and `origin/fork` is retired.
- Fork-specific verification runs locally through the checked-in harness and pre-push hook; no
  fork-specific GitHub Actions check is added.
- Latest stable means the greatest globally published `rust-vX.Y.Z` tag; patch releases may live on
  an upstream release branch rather than `main`.
- Resolution occurs during sync and release preflight, then the result is committed for reproducible
  builds.
- A completed `roll --run` hands publication to `publish --publish`; rebuilding is reserved for a
  one-step roll whose publication was authorized before the build began. The handoff preserves the
  prepared date and output path so UTC rollover or a custom directory cannot select other assets.
- Rusty V8 cache entries are scoped by exact release tag and target, and checksum validity—not
  presence alone—decides whether an entry is reusable.

## Boundary

Force-pushing `origin/main`, creating or pushing tags, GitHub release publication, live credentials,
and any replacement of an already published artifact require explicit human authorization for the
named ref or artifact.
