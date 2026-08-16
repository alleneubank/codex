## Fork maintenance policy

- Maintain the fork product and release source only on local `main` and `origin/main`.
- Rebase `main` onto `upstream/main`; do not recreate or maintain `origin/fork`.
- Classify every fork commit before creating it. Upstream-bound work has an unprefixed conventional
  subject and contains only code suitable for an upstream PR. Fork-only release, distribution, or
  standing-law maintenance on `main` uses a `[fork]` prefix before its conventional subject (for
  example, `[fork] chore(release): refresh fork metadata`) and is excluded from upstream PRs.
- Amend, do not accrete, while iterating on unmerged work: review feedback, dogfood fixes, and
  rebase resolution rewrite the existing feature commit with `git commit --amend` or a deliberate
  history rewrite instead of stacking work-in-progress `fix:` commits. Use `fix:` only for a real
  defect in code that has already merged upstream.
- Before replaying fork commits during an upstream sync, inspect the upstream log and diff since
  the prior merge base. Classify each fork commit as **drop** (upstream supersedes it), **adopt**
  (use upstream's implementation and retain only fork policy), or **adapt** (rewrite it against
  upstream's current extension points, types, permission/environment models, lifecycle APIs, and
  release/build conventions). Make that classification before resolving conflicts; never preserve
  a stale compatibility projection because it compiles. Verify each adaptation under the minimal
  verification rule below.
- After every upstream sync, run `just update-fork-version`, commit the changed pin, and run
  `just check-fork-version`. Run `just test-fork-maintenance` only when fork maintenance or release
  scripts change.
- Install the repository-owned pre-push policy with `just install-fork-hooks`. It checks the exact
  pushed commit and fails closed when the stable version pin is stale.
- Build and verify fork releases locally. Do not add fork-specific GitHub Actions checks.
- Use `bash .github/scripts/fork-release.sh roll` for the dry-run release plan. Tags, release assets,
  and publication remain immutable and require explicit human authorization.
- This personal fork prioritizes minimal verification. For fork pruning and upstream adaptations,
  remove tests and snapshots owned only by removed features. Keep unrelated upstream tests in place.
  Compile affected crates with `cargo check`, then do one live smoke with the cheapest suitable
  model and reasoning settings for behavior the compiler cannot show. Run focused tests only for
  a specific remaining risk; do not run crate or workspace suites by default. This rule supersedes
  other test-execution, test-authoring, and snapshot guidance here for fork-only maintenance.

