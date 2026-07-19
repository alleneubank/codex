## Fork maintenance policy

- Maintain the fork product and release source only on local `main` and `origin/main`.
- Rebase `main` onto `upstream/main`; do not recreate or maintain `origin/fork`.
- After every upstream sync, run `just update-fork-version`, commit the changed pin, and run
  `just test-fork-maintenance` plus `just check-fork-version`.
- Install the repository-owned pre-push policy with `just install-fork-hooks`. It checks the exact
  pushed commit and fails closed when the stable version pin is stale.
- Build and verify fork releases locally. Do not add fork-specific GitHub Actions checks.
- Use `bash .github/scripts/fork-release.sh roll` for the dry-run release plan. Tags, release assets,
  and publication remain immutable and require explicit human authorization.

