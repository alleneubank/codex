# TUI Permission-Mode Switching Quality Law

Law doc for TUI permission-mode switching, present-tense, no narrated history — git is the
changelog. The Boundary and ratified Decisions amend only with human confirmation; the driver
appends provisional Decisions, marked and dated.

## Bar

The permission shortcut is shippable when every allowed built-in mode is keyboard-reachable
without weakening confirmation, managed-policy, source-thread, session-only, or diagnostic
behavior.

## Dimensions

- Security: elevated authority remains behind explicit confirmation and managed requirements.
- Correctness: next and previous cycling agree on the allowed built-in mode set.
- Scope: a confirmed selection changes only its originating displayed thread and never the saved
  default.
- Resilience: cancellation, stale events, and repeated keys cannot wedge or duplicate the flow.
- Observability: every attempted app-server update produces one actionable success or failure
  history cell.

## Floors

- Forward and reverse shortcut tests select Full Access when it is allowed and skip it when managed
  requirements forbid it.
- Full Access shortcut tests observe a confirmation event before any apply event, including under
  repeated key input.
- Confirmation tests prove that cancel preserves the current mode and permits a later shortcut.
- App integration tests assert the complete app-server permission settings, active profile, app and
  widget reviewer state, originating thread id, unchanged `config.toml`, and absence of duplicate
  events after acceptance.
- Existing permission-popup snapshot coverage remains green with no pending snapshots.
- The focused `codex-tui` permission-shortcut and permission-popup tests pass under `just test`.

## Oracle

The objective oracle is the repository's deterministic Rust harness: chat-widget tests inspect the
event boundary, and embedded app-server integration tests inspect the resulting thread settings,
in-memory configuration, history output, and on-disk config. These checks bind the result to
observable state rather than the implementer's description; nextest reports the verdict.

## Never

- Never apply Full Access before the user accepts the existing Full Access confirmation.
- Never persist a shortcut selection to `config.toml`.
- Never let a stale source-thread event mutate a different displayed thread.
- Never offer a mode forbidden by managed approval, reviewer, or profile requirements.
- Never queue duplicate confirmation or apply events from repeated shortcut input.

## Decisions

- 2026-08-23 — ratified: Full Access participates in the built-in shortcut cycle.
- 2026-08-23 — provisional: The existing Full Access confirmation remains the authority boundary,
  and acceptance continues through the shortcut's source-thread app-server update path. This keeps
  shortcut behavior session-only and avoids a second elevation mechanism.
- Security and managed requirements outrank shortcut convenience.

## Boundary

Publishing, pushing, merging, or weakening the Full Access confirmation or managed-policy floors
requires human authorization. Implementing and verifying the session-only shortcut path remains
interior work.
