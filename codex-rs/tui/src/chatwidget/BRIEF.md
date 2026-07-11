> Law doc for prompt stash, present-tense, no narrated history — git is the changelog. Amend
> Decisions and Boundary only with human confirmation; log the rationale.

## Bar

Prompt stash is shippable when a preserved draft remains visibly discoverable, then returns
immediately and losslessly after accepted intervening input without ever replacing input the user
still needs to fix.

## Dimensions

- Timing: restoration happens at the accepted-submission boundary.
- Losslessness: every composer field and subsequent edit survives.
- Safety: rejected or non-empty input is never overwritten.
- Visibility: stash presence is legible without competing with editable input or status surfaces.
- Integration: prompts, shell input, slash commands, queues, and thread snapshots remain coherent.

## Floors

- Focused `ChatWidget` tests exercise accepted prompt, accepted command, rejection, and later turn
  completion.
- Full-widget snapshots exercise a long replacement draft, the adaptive labels, a command-backed
  status line, and a nonzero ambient-pet reservation at exact width boundaries.
- The complete `codex-tui` test suite and scoped lint pass.
- An interactive `just codex` run visibly restores the draft before the submitted work completes.

## Oracle

Deterministic state assertions and snapshots judge the exact composer contents at each lifecycle
boundary. A fresh structured reviewer judges the lifecycle and layout evidence independently. The
real TUI walkthrough is the final proxy check because it exercises terminal input, dispatch, and
rendering together rather than calling restoration helpers directly.

## Never

- Never wait for turn completion after input was accepted.
- Never overwrite rejected input or a new draft.
- Never discard rich composer state or restore the same stash twice.
- Never narrow or cover editable input, the custom status line, or ambient companion space merely
  to display stash presence.
- Never change queue, command, or turn semantics merely to make the stash test pass.

## Decisions

- Restoration follows successful submission, not the last server turn.
- A prompt restored while work runs remains editable and is never auto-submitted.
- Validation failures and rejected submissions keep the intervening input intact.
- Stash presence renders in the composer's existing top padding as `draft stashed`, compacts to
  `stashed`, then disappears rather than colliding.
- Custom status lines remain independent; stash state is not added to their payload.
- The local replayed feature commit is amended; published `origin/main` is unchanged.

## Boundary

Local implementation, verification, and commit amendment are authorized. Pushes, force-pushes,
pull requests, and publication remain human actions requiring separate authorization.
