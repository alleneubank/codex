# Pending Steer Editing Quality Law

Law doc for pending-steer editing, present-tense, no narrated history — git is the changelog. The
Boundary and ratified Decisions amend only with human confirmation; the driver appends provisional
Decisions, marked and dated. Durable contract traceability and verification evidence live in the
colocated SPEC.

## Bar

Pending-steer editing is shippable when plain Up safely returns the latest still-undelivered steer
to the composer without interrupting the agent or allowing the original message to reach the model.

## Dimensions

- Correctness: Core delivery and withdrawal have one atomic winner.
- Fidelity: the complete model-significant rich user message returns unchanged, input entered
  while withdrawal is in flight is preserved, and the editor reopens at a defined cursor position.
- Interaction: contextual Up preserves existing paste-burst, history, cursor, popup, and modal
  ownership.
- Isolation: stale events cannot affect another thread, turn, or identical-looking steer.
- Resilience: late, rejected, repeated, and failed requests neither duplicate nor misrepresent
  input.
- Observability: one actionable warning identifies a failed withdrawal without claiming the draft
  is safe to edit.

## Floors

- A deterministic Core test gates withdrawal against pending-input drain in both orders and asserts
  the complete delivered input.
- App-server v2 tests exercise the public JSON-RPC boundary, including exact-id success, wrong-turn
  rejection, missing-id rejection, and unchanged ordinary steer append semantics.
- Stable/experimental schema tests prove that method-level experimental gating excludes the RPC
  from stable exports and includes its method plus all typed payloads in experimental exports.
- TUI tests assert the emitted source-thread request, delayed restore until success, complete
  `UserMessage` equality, active and off-screen composer-state preservation, paste-burst flushing,
  event-gap merge order, cursor-at-end behavior, identical-message identity, and absence of
  interrupt/start/steer fallback.
- Key-routing tests keep repeat-key, nonempty-composer, paste-burst, popup/modal, no-pending,
  multiline-cursor, and ordinary history cases unchanged.
- Pending-state tests prove that repeated Up emits one request, only the matching request id can
  complete it, and transport uncertainty stays non-editable until an authoritative lifecycle
  signal reconciles it.
- Submission-state tests cover successful steer, mismatch retry, start fallback success and
  rejection, non-steerable and generic rejection, transport/deserialization uncertainty,
  commit-before-response, turn-end, interrupt, replay, and off-screen completion.
- A transition-table test asserts the next state and full observable effect for every row in the
  SPEC, including the deliberately retained non-editable row after uncertain withdrawal and normal
  completion.
- The pending-input preview snapshot advertises plain Up only where the action is available, with no
  unreviewed pending snapshots.
- Every touched crate's focused `just test -p ...` suite, app-server schema generation checks,
  scoped lint/fix, and repository formatting pass.
- An in-process full-path test drives TUI request dispatch through app-server into Core for both
  withdrawal-wins and drain-wins, and a recorded live TUI smoke covers contextual Up, a nonempty
  composer, and a thread switch while withdrawal is pending.

## Oracle

The objective oracle is the repository harness: Core races are deterministically gated, app-server
integration tests inspect public JSON-RPC behavior and model-bound requests, and TUI tests inspect
the event and rendered-output boundaries. A fresh-context reviewer applies the SPEC and this brief
with a `major` severity floor; the author does not approve the work.

## Never

- Never restore a pending steer optimistically before Core confirms withdrawal.
- Never interrupt the active turn to implement editing.
- Never fall back to starting or steering another turn after withdrawal fails.
- Never match an editable steer by display text when a stable client id is available.
- Never redirect a stale edit request to the currently displayed thread.
- Never let a late or duplicate withdrawal response mutate a row in a different local lifecycle
  state.
- Never mutate the displayed widget with a completion owned by an off-screen source thread.
- Never restore a withdrawal-uncertain row on normal completion when Core delivery cannot be
  distinguished from successful withdrawal.

## Decisions

- 2026-08-27 — ratified: Plain Up is the user-facing edit gesture.
- 2026-08-28 — ratified: Atomic withdrawal precedes composer restoration because editing time is
  unbounded.
- 2026-08-28 — ratified: Correctness and isolation outrank optimistic responsiveness; failure
  leaves the pending row intact and emits a warning.

## Boundary

The additive experimental app-server contract is ratified for this campaign. Publishing, pushing,
opening a pull request, merging, or weakening an invariant requires human authorization.
Implementing, testing, and independently reviewing the bounded change are interior work.
