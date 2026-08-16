# Fork maintenance

`origin/main` is the only maintained product branch for this fork. It contains the fork commits on
top of `upstream/main`; `origin/fork` is retired and must not be recreated.

After syncing upstream, update and commit the stable version pin, then run the maintenance and
freshness checks:

```sh
git fetch --prune origin
git fetch --prune --tags upstream
previous_base="$(git merge-base main upstream/main)"
git log --oneline "$previous_base"..upstream/main
git diff --stat "$previous_base"..upstream/main
git rebase --update-refs upstream/main
just update-fork-version
git diff --quiet HEAD -- codex-rs/fork-version.txt || {
  git add codex-rs/fork-version.txt
  git commit -m "[fork] chore(release): update upstream version pin"
}
just test-fork-maintenance
just check-fork-version
```

Before replaying commits, classify each fork commit as **drop** when upstream supersedes it,
**adopt** when upstream's implementation should replace it while fork policy remains, or **adapt**
when the fork behavior must be rewritten against upstream's current extension points, types,
permission/environment models, lifecycle APIs, and release/build conventions. Make that decision
before resolving conflicts. Add focused regression coverage for every adaptation; do not preserve
stale compatibility code merely because it compiles.

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
