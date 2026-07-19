# Fork maintenance

`origin/main` is the only maintained product branch for this fork. It contains the fork commits on
top of `upstream/main`; `origin/fork` is retired and must not be recreated.

After syncing upstream, update and verify the stable version pin before committing:

```sh
git fetch --prune origin
git fetch --prune --tags upstream
git rebase upstream/main
just update-fork-version
just test-fork-maintenance
just check-fork-version
```

Install the repository-owned push policy once per clone:

```sh
just install-fork-hooks
```

The pre-push hook rejects deletion of `origin/main`, rejects non-deletion pushes to
`origin/fork`, and checks `codex-rs/fork-version.txt` from the exact commit being pushed to
`origin/main`. Stable version discovery happens only during maintenance and release preflight;
ordinary builds remain network-independent.

Fork releases are built and verified locally from `main`:

```sh
bash .github/scripts/fork-release.sh roll
```

The dry run prints the immutable candidate identity and the explicit build and publication
commands. Publishing remains a separate human-authorized action.
