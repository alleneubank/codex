# Pending Steer Editing

The TUI accepts follow-up messages while an agent turn is running. Those messages are immediately
submitted to Core as pending steers and are shown in the pending-input preview until Core commits
them at the next sampling boundary. The existing queued-message edit shortcut only covers input
that has not been submitted to Core, so changing the TUI preview alone would let the original text
reach the model.

Pending-steer editing withdraws the exact still-pending Core input before restoring its retained
rich user-message payload. This gives the user time to edit without interrupting the running turn
and without leaving the original text eligible for delivery.

## Domain model

- A `PendingSteer` is the TUI's local mirror of one user input accepted into the active turn's Core
  pending-input queue.
- Every TUI-originated pending steer carries an opaque client user-message id and the active turn id
  that accepted it.
- A withdrawal identifies one pending steer by thread id, expected turn id, and client
  user-message id.
- Core withdrawal and Core pending-input drain contend on the same lock. Exactly one wins: either
  the input is withdrawn for editing or it is committed for model delivery.
- A pending steer moves through explicit local states: awaiting steer acceptance, acceptance
  uncertain, accepted by one turn, awaiting commit after a start fallback, withdrawal in flight,
  and withdrawal uncertain after a transport failure. Only an accepted steer is eligible for Up;
  repeated Up is consumed while a withdrawal is in flight or uncertain. An active non-bracketed
  paste burst retains normal composer routing and is not eligible for withdrawal.
- The TUI keeps the local pending steer unchanged while withdrawal is in flight. A correlated
  successful response removes it and restores the draft; a rejection leaves it pending.

## Pending-steer transitions

Every transition is keyed by source thread id and client user-message id. Withdrawal transitions
also require the accepted turn id and local withdrawal request id to match.

| Current state | Event | Next state and observable effect |
| --- | --- | --- |
| `AwaitingAcceptance` | steer succeeds, including after mismatch retry | `Accepted { response_turn_id }`; the response turn, not the originally attempted turn, owns the row. |
| `AwaitingAcceptance` | start fallback succeeds | `AwaitingCommitAfterStart { response_turn_id }`; Up is disabled until the ID-matched user item commits and removes the row. |
| `AwaitingAcceptance` | definitive start, non-steerable, or generic JSON-RPC rejection | Remove the exact row and pass its retained message/history through the existing rejected-steer recovery path; emit the existing error. |
| `AwaitingAcceptance` | submission transport or deserialization failure | `AcceptanceUncertain`; retain the row, disable Up, and emit one uncertainty warning. |
| `AwaitingAcceptance` or `AcceptanceUncertain` | ID-matched commit arrives before a submission response | Remove and render the row; a later response is a no-op. |
| `Accepted` | eligible plain Up | `WithdrawalInFlight { accepted_turn_id, request_id }`; emit exactly one source-thread withdrawal request. |
| `WithdrawalInFlight` | repeated Up | Remain in flight, consume the key, and emit nothing. |
| `WithdrawalInFlight` | matching success | Remove the row, flush any active paste burst, and prepend `user_message_for_restore(message, history_record)` to composer input entered while the request was in flight. Preserve that input's rich payload and pending pastes, with the cursor at the end of the merged draft. |
| `WithdrawalInFlight` | matching JSON-RPC rejection | Return to `Accepted`, retain the row, and emit one warning. |
| `WithdrawalInFlight` | transport or deserialization failure | `WithdrawalUncertain`; retain the row, disable Up, and emit one uncertainty warning. |
| Any pending state | ID-matched commit | Remove and render the row, invalidate its request id, and make later responses no-ops. |
| `Accepted` or an acceptance state | normal turn completion without an ID-matched commit | Retain the row and its state; Core may carry pending input into its next sampling turn. |
| `WithdrawalInFlight` | normal turn completion before the request result | Retain the in-flight row; only its matching result may resolve it. |
| `WithdrawalUncertain` | normal turn completion or replay without an ID-matched commit | Retain the row as non-editable and do not restore it; the existing warning remains the actionable signal because delivery versus withdrawal is unknown. |
| Any pending state | confirmed turn interruption | Use the existing interrupted-turn restore policy, invalidate its request id, and ignore later responses. |
| Any pending state | replay with an ID-matched committed user item | Remove the row; replay without a match preserves it only when in-flight input state itself is preserved. |

## Requirements

- **REQ-PENDING-STEER-001**: Plain Up requests editing of the newest TUI-owned pending steer only
  when the composer is empty, no non-bracketed paste burst is active, and no popup or modal owns
  the key. Press and repeat key events follow the same eligibility rules.
- **REQ-PENDING-STEER-002**: Editing restores the pending steer's text, local and remote images,
  text elements, mention bindings, and history representation without interrupting the active
  turn. If composer input arrives while withdrawal is in flight, the withdrawn message is prepended
  to that input and the existing rich payload and pending pastes are preserved. The cursor is placed
  at the end of the merged draft; transient cursor and paste-burst state from the original
  submission are not reconstructed.
- **REQ-PENDING-STEER-003**: Core removes the exact pending user input atomically by expected turn
  id and client user-message id before the TUI removes its local mirror or restores the draft.
- **REQ-PENDING-STEER-004**: A missing active turn, turn-id mismatch, missing id, or ambiguous id
  fails closed without removing another input, appending replacement input, or restoring an
  editable draft whose original may still be delivered.
- **REQ-PENDING-STEER-005**: After successful withdrawal, submitting the edited draft uses the
  normal user-turn path with a new client user-message id; Core can receive only that resubmitted
  version of the withdrawn steer.
- **REQ-PENDING-STEER-006**: Pending-steer commit notifications reconcile by client user-message id
  so identical message text and images cannot remove the wrong local preview row.
- **REQ-PENDING-STEER-007**: With no eligible pending steer, a nonempty composer, an active
  non-bracketed paste burst, or an active popup or modal, plain Up retains its existing paste,
  cursor, history, or popup behavior.
- **REQ-PENDING-STEER-008**: Withdrawal is scoped to the thread and turn that originated the key
  press; switching the displayed thread cannot redirect the request.
- **REQ-PENDING-STEER-009**: A withdrawal rejection or transport failure leaves the local pending
  row unchanged and emits one actionable warning rather than optimistically editing or duplicating
  the message.
- **REQ-PENDING-STEER-010**: Every withdrawal carries a local request id. Only a response matching
  the source thread id, accepted turn id, client user-message id, and current request id may mutate
  the pending row; repeated Up and stale responses cannot issue or complete a second edit.
- **REQ-PENDING-STEER-011**: A transport failure enters an uncertain state that remains
  non-editable until a commit, confirmed interruption, or replay evidence reconciles the row.
  Normal turn completion alone does not resolve uncertainty. A JSON-RPC rejection is definitive
  and may return the row to its accepted state without restoring it.
- **REQ-PENDING-STEER-012**: Submission outcomes reconcile the exact row by client user-message id:
  steer success, including a turn-mismatch retry, records the response turn id; start fallback
  success awaits its commit item and is not editable; definitive start, non-steerable, or generic
  JSON-RPC rejection follows the existing rejected-steer restore path; transport or deserialization
  failure becomes acceptance-uncertain; commit-before-response removes the row and makes the late
  response a no-op.
- **REQ-PENDING-STEER-013**: Turn-end, interrupt, and replay reuse their existing pending-steer
  reconciliation policy, invalidate any local withdrawal request id for a moved or removed row, and
  make later responses no-ops.
- **REQ-PENDING-STEER-014**: When a response is processed while its source thread is off-screen, it
  mutates that thread's stored `ThreadInputState`; it never mutates the displayed `ChatWidget`.

## Invariants

- A pending steer is never editable in the composer while its original remains eligible for Core
  delivery.
- Pending-steer editing never emits `turn/interrupt` and never falls back to `turn/start` or an
  ordinary `turn/steer` when withdrawal fails.
- Existing end-of-turn queued-message editing remains on its configured dedicated binding.
- Core's pending-input delivery order is unchanged for inputs that are not withdrawn.

## Non-goals

- Editing input that Core already drained or committed is not supported.
- The change does not add arbitrary editing or deletion of transcript history.
- The change does not redefine repeated client user-message ids on ordinary `turn/steer` requests
  as updates.
- The change does not persist an in-progress draft across a TUI process crash.
- The change does not reproduce the cursor position or pending-paste placeholders that existed
  before the original submission expanded and cleared them.

## Decisions

- 2026-08-27 — ratified: Plain Up is the requested editing gesture.
- 2026-08-28 — ratified: The TUI withdraws the pending steer on Up and restores it only after
  success, instead of waiting to replace it on Enter. This closes the delivery race while the user
  edits.
- 2026-08-28 — ratified: The wire contract is an additive experimental
  `turn/withdrawPendingInput` method. Ordinary `turn/steer` append semantics remain unchanged.

## App-server v2 contract

`turn/withdrawPendingInput` is experimental and has one resource/method path. Validation, loading,
policy, and operational outcomes are exhaustive:

- Request: `TurnWithdrawPendingInputParams { threadId, expectedTurnId,
  clientUserMessageId }`, where every string must be nonempty.
- Success: `TurnWithdrawPendingInputResponse { turnId }` after exactly one pending user input is
  removed.
- Operational rejection after validation and thread loading uses JSON-RPC code `-32600` with `data`
  shaped as
  `TurnWithdrawPendingInputError { reason, expectedTurnId, actualTurnId }`. `reason` is one of
  `noActiveTurn`, `expectedTurnMismatch`, `notPending`, or `ambiguousClientUserMessageId`; the two
  turn-id fields are nullable and always present.
- Ordinary `turn/steer` retains append semantics even when a client user-message id repeats.

| Order | Outcome | Code | Message | Data |
| --- | --- | --- | --- | --- |
| 1 | empty `threadId` | `-32600` | `threadId must not be empty` | `null` |
| 2 | empty `expectedTurnId` | `-32600` | `expectedTurnId must not be empty` | `null` |
| 3 | empty `clientUserMessageId` | `-32600` | `clientUserMessageId must not be empty` | `null` |
| 4 | malformed `threadId` | `-32600` | `invalid thread id` | `null` |
| 5 | valid but unloaded thread | `-32600` | `thread not found: {threadId}` | `null` |
| 6 | direct input forbidden by multi-agent v2 policy | `-32600` | `direct app-server input is not allowed for multi-agent v2 sub-agents` | `null` |
| 7 | no active turn | `-32600` | `no active turn contains pending input` | `{ reason: "noActiveTurn", expectedTurnId: <request>, actualTurnId: null }` |
| 8 | active turn differs | `-32600` | `expected active turn id \`{expected}\` but found \`{actual}\`` | `{ reason: "expectedTurnMismatch", expectedTurnId: <expected>, actualTurnId: <actual> }` |
| 9 | no pending user input has the client id | `-32600` | `client user message id is not pending` | `{ reason: "notPending", expectedTurnId: <expected>, actualTurnId: <actual> }` |
| 10 | multiple pending user inputs have the client id | `-32600` | `client user message id matches multiple pending inputs` | `{ reason: "ambiguousClientUserMessageId", expectedTurnId: <expected>, actualTurnId: <actual> }` |

## Risk

**High risk — public API contract.** The additive experimental app-server request and response
types are ratified for this campaign. The contract remains explicit and does not overload ordinary
steer retries or repeated client ids.

## Acceptance criteria

- [x] Empty-composer plain Up withdraws and restores the newest still-pending steer without
  interrupting.
- [x] The original text cannot reach the model after withdrawal succeeds.
- [x] A drain that wins the race causes withdrawal to reject without appending or restoring input.
- [x] The complete model-significant rich user message survives the round trip; input entered while
  withdrawal is in flight is preserved for active and off-screen threads, and the restored cursor
  is at the end of the merged draft.
- [x] Identical pending messages reconcile and edit by client id, not content comparison.
- [x] Nonempty-composer, paste-burst, popup, modal, no-pending, thread-switch, and interrupt behavior
  retain their existing semantics.
- [x] Every submission, withdrawal, lifecycle, off-screen, and stale-response transition has an
  observable deterministic test.
- [x] Every row of the app-server outcome table has a wire-level assertion.
- [x] The stable schema omits the experimental method; the experimental schema and generated
  TypeScript include the method, params, response, and structured rejection reasons.
- [x] A deterministic in-process TUI → app-server → Core test proves withdrawal success and a
  drain-wins rejection through the real request path.

## Test traceability

| Contract | Primary verifier |
| --- | --- |
| REQ-PENDING-STEER-001, 007 | `tui/src/chatwidget/tests/pending_steer_edit.rs` plain-Up press/repeat routing, binding-collision, history recall, multiline cursor, paste-burst, popup/modal, and no-eligible-row cases; pending-input preview snapshots. |
| REQ-PENDING-STEER-002, 005 | Rich-message restoration and resubmission tests deep-compare the retained message, merged active/off-screen composer state, pending pastes, event-gap ordering, cursor, and fresh resubmission identity. |
| REQ-PENDING-STEER-003, 004 | Core queue race test plus `core/tests/suite/pending_input.rs` exact-match, queue-order, wrong-turn, missing, duplicate-id, no-active-turn, withdrawal-wins, and drain-wins cases. |
| REQ-PENDING-STEER-006 | TUI commit/replay tests reconcile identical display payloads by `client_user_message_id`; ordinary repeated-id steer append behavior remains covered at the app-server boundary. |
| REQ-PENDING-STEER-008, 010, 014 | Source-thread dispatch and off-screen completion tests correlate thread, turn, client-message, and request ids and reject stale or redirected responses. |
| REQ-PENDING-STEER-009, 011 | TUI rejection, repeat-key Up, transport/deserialization uncertainty, completion, interruption, replay, and late-response cases prove fail-closed retention and recovery. |
| REQ-PENDING-STEER-012, 013 | Submission and lifecycle transition-table tests cover steer acceptance and retry, start fallback, rejection, uncertainty, commit-before-response, turn end, interruption, replay, and late no-ops. |
| Experimental app-server contract | `app-server/tests/suite/v2/turn_withdraw_pending_input.rs` asserts validation order, loading and policy rejection, every typed operational outcome, success, and drain-wins; `turn_steer.rs` preserves ordinary steer append semantics. `pending_input_withdrawal_exports_are_method_gated` asserts stable/experimental schema boundaries, generated dependencies, and always-present nullable error fields. |

## Verification evidence

- Schema generation: stable and experimental `just write-app-server-schema` passes with no
  uncommitted fixture drift.
- Focused suites: Core pending input 27/27, Core exhaustive elicitation adaptation 1/1,
  app-server withdrawal 3/3, the pre-review TUI pending-steer set 46/46, the post-review focused
  pending-steer-edit set 20/20, and app-server protocol 299/299 pass.
- Full TUI suite: the pre-review tree passes 4100/4100. The post-review tree runs 4111 tests with
  4106 passing and five unrelated lifecycle timeouts; every pending-steer unit and integration test
  passes. Three lifecycle cases pass on immediate serial rerun, while the two remaining timeout-
  prone cases pass on exact clean upstream. No pending `insta` snapshots remain.
- Full Core suite: 4006 pass and four unrelated failures reproduce on the exact clean
  `upstream/main` tree (`6478a751fd`).
- Full app-server suite: all withdrawal tests pass; the unrelated code-mode, provenance,
  filesystem-watch, image-generation, and MCP-status failures also reproduce or exhibit the same
  flakiness on the exact clean upstream tree.
- Complete workspace `just test`: both this branch and the exact clean upstream tree stop at the
  same unavailable `rusty_v8` 150.4.0 aarch64 macOS archive (HTTP 404) before the workspace suite
  can execute.
- Fork gates: `just test-fork-maintenance` and `just check-fork-version` pass with upstream version
  pin 0.151.0.
- Live TUI smoke: while a `sleep 20` turn was active, an accepted pending steer displayed the new
  edit hint; plain Up issued `turn/withdrawPendingInput`, removed the pending preview only after
  success, restored the exact draft, and allowed the original turn to finish without receiving the
  withdrawn text.
