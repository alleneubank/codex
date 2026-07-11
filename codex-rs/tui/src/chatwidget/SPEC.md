# Prompt Stash

## Problem and solution

Prompt stash preserves a draft while the user sends a one-off prompt or command. A persistent
composer indicator keeps that hidden draft discoverable while the stash exists. The preserved draft
returns as soon as the intervening input is accepted, so the user can continue editing while the
resulting work runs. Restoration is coupled to input acceptance, not to server turn IDs or turn
completion.

## Domain model

- `PromptStash` owns a complete `ThreadComposerState`, including text, attachments, mentions,
  pending pastes, and cursor position.
- `InputResult` is the UI boundary between composer validation and `ChatWidget` dispatch.
- A submission is accepted when a prompt is queued or handed to `submit_op`, or when a parsed
  command is handed to its application-level dispatcher.
- `ChatWidget::prompt_stash` remains authoritative; `ChatComposer` receives only a presentation
  projection of whether the active thread owns a stash.

## Requirements

- **REQ-STASH-001:** Toggling stash on a non-empty composer captures the complete composer state
  and clears the composer; toggling again restores it only when the composer is empty.
- **REQ-STASH-002:** An accepted prompt, queued prompt, shell prompt, or parsed command restores the
  stashed draft immediately after dispatch, without waiting for turn start or completion.
- **REQ-STASH-003:** Validation failures and rejected prompt submissions preserve the intervening
  input and do not overwrite it with the stash.
- **REQ-STASH-004:** Restoring a stash is lossless for text, attachments, mentions, pending pastes,
  and cursor position.
- **REQ-STASH-005:** Thread input snapshots preserve a stash across thread switches and replay.
- **REQ-STASH-006:** While the active thread owns a stash, the visible composer shows an indicator
  whether the current composer is empty or contains an intervening draft.
- **REQ-STASH-007:** At sufficient width the indicator reads `draft stashed` in subdued cyan,
  right-aligned in the composer's existing top padding. It compacts to `stashed`, then disappears
  when neither label fits.
- **REQ-STASH-008:** The indicator does not change composer height, textarea width, prompt wrapping,
  or cursor placement, and it respects the ambient pet's right-side reservation.
- **REQ-STASH-009:** Manual restoration, accepted-submission restoration, and thread-state clearing
  remove the indicator in the same transition that consumes or clears the stash.
- **REQ-STASH-010:** Built-in and command-backed status lines, footer hints, keybindings, and the
  custom-status-line payload remain unchanged.

## Invariants

- Automatic restoration never overwrites a non-empty composer.
- Turn lifecycle notifications do not trigger stash restoration.
- A restored stash is consumed exactly once.
- Presentation state never determines whether a rich draft can be restored.

## Non-goals

- Changing slash-command availability or validation rules.
- Changing prompt queue ordering, steering, or turn lifecycle behavior.
- Publishing or force-pushing rewritten history.

## Acceptance criteria

- [x] A stashed draft is visible immediately after an accepted prompt while its turn is pending or
      running.
- [x] A stashed draft is visible immediately after an accepted slash command.
- [x] Rejected input remains editable and the stash remains available.
- [x] Later turn completion does not replace edits made to the restored draft.
- [x] Full, compact, and omitted indicator states render without changing replacement-draft
      wrapping or the custom status line.
- [x] A nonzero ambient-pet reservation shifts the indicator's exact compaction and omission
      boundaries without overlap.
- [x] Manual, accepted-submission, and thread-state restoration clear the indicator.
- [x] Focused tests, the `codex-tui` suite, formatting, linting, and an interactive TUI walkthrough
      pass.

## Test traceability

- `chatwidget::tests::prompt_stash::accepted_prompt_queue_shell_and_command_restore_stash_immediately`
  covers REQ-STASH-002 and REQ-STASH-009 for accepted inputs.
- `chatwidget::tests::prompt_stash::rejected_prompt_keeps_intervening_input_and_stash` covers
  REQ-STASH-003.
- `chatwidget::tests::prompt_stash::manual_stash_is_lossless_non_overwriting_and_empty_stash_is_a_no_op`
  covers REQ-STASH-001 and REQ-STASH-004.
- `chatwidget::tests::prompt_stash::prompt_stash_indicator_tracks_presence_and_composer_layout`
  covers REQ-STASH-005 through REQ-STASH-010 with full-widget renders, exact reserved-width
  boundaries, lifecycle assertions, and reviewed snapshots.
