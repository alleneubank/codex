# Loop: Edit pending TUI messages without interrupting — `main`

Mission: drive this branch to **interior-green** so the only remaining steps are the human's
boundary calls (reviewing the result, pushing, and proposing/merging upstream). Work through the
ADF loop (SPEC → PLAN → TDD → DEV → E2E). Unblock via the ladder; the verifier — not confidence —
decides when work is done.

## State (updated 2026-08-28 — rewrite each iteration; newest facts first)

- Branch `main`, HEAD `2d4418d0f4`, tree dirty only for this charter. Nothing pushed.
- Local `main` and the stacked mission ref are rebased onto live `upstream/main`; obsolete fork pins
  are dropped, retained fork commits are adapted to current extension points, fork-only commit
  subjects are classified, and annotated pre-rewrite backup refs preserve every prior tip.
- `just update-fork-version` produced the committed `0.150.1` pin. `just
  test-fork-maintenance` and `just check-fork-version` pass against the rebased history.
- The reported preview says “Messages to be submitted after next tool call”; source inspection
  proves those rows are TUI `pending_steers`, already accepted into Core's active-turn
  `TurnInputQueue`, not the editable end-of-turn `queued_user_messages` queue.
- Plain Up is the editor/history binding. The existing `chat.edit_queued_message` action defaults to
  Alt+Up/Shift+Left and only pops `queued_user_messages` or rejected steers; it cannot update an
  accepted pending steer.
- Recall and GitHub issue lookup were attempted, but sandboxed socket/network access prevented
  either lookup. Local source, history, tests, and the supplied screenshots remain available.
- A fresh-context design review recommends withdrawing the exact pending input on Up before
  restoring the composer. Replacement on Enter remains racy because editing time is unbounded.
- The smallest race-free implementation adds an experimental `turn/withdrawPendingInput` v2
  contract. The user ratified that public API contract and authorized the complete workspace test
  run for this campaign.
- Fresh-context plan review round 1 returned `REVISE` with five major findings. The plan now uses a
  resource/method RPC name and structured errors, lists the full identity/turn plumbing, defines an
  in-flight/uncertain lifecycle, narrows restoration to state the current submission seam retains,
  and makes Core model-bound integration coverage mandatory.
- Fresh-context plan review round 2 returned `REVISE` with four major findings. The plan now defines
  validation and lookup error order, includes exhaustive request dispatch and the Core public
  export, enumerates every submission/lifecycle transition, and assigns off-screen completion plus
  new RPC plumbing to focused modules instead of growing oversized central files.
- Fresh-context plan review round 3 returned `REVISE` with five major findings. Its findings are
  incorporated into the handoff: the SPEC now has exhaustive wire and lifecycle tables; the plan
  defines active/channel/overview state ownership and nonblocking request dispatch; pending-steer
  state moves to its own module; and override fidelity has separate retained-row and restored-editor
  assertions. The three-round plan-review budget is exhausted, so a new fresh plan gate is required
  after human PLAN approval and before TDD.
- The fresh-context PLAN gate required before TDD is complete. No pending-steer runtime code,
  tests, or schemas have changed yet.
- Fresh-context PLAN review first returned `REVISE` for experimental export gating, the exact Core
  API, one TUI module declaration, and a missing full-path E2E floor. After those amendments, a
  second fresh-context review found one explicit error-dependency export gap; adding
  `export.rs`/fixture coverage and the dependency registry resolved it. The final bounded verdict
  is `APPROVE` with no major-or-higher findings.

## Decisions (append-only; do not re-litigate)

1. 2026-08-27 — Plain Up may edit only when the composer is empty, no modal/popup owns input, and
   a pending message exists; otherwise normal cursor/history navigation remains unchanged. Why:
   this is the narrow contextual interpretation of the user's explicit request. provisional
   (driver)
2. 2026-08-27 — A preview-only edit is invalid: Core must atomically replace or reject replacement
   of the undelivered input so the typo cannot still be sent. Why: the observable contract is what
   reaches the model, not what the TUI renders. provisional (driver)
3. 2026-08-27 — Withdraw the exact pending steer on Up and restore it only after Core confirms
   success. Why: waiting to replace on Enter leaves the original eligible for delivery throughout
   an unbounded edit interval. provisional (fresh-context design review + driver)
4. 2026-08-27 — Use a separate experimental `turn/withdrawPendingInput` RPC rather than changing
   repeated-id semantics or adding a mode to `turn/steer`. Why: ordinary steer must remain append,
   and withdrawal must never inherit start/steer retry fallbacks. provisional; **PLAN approval
   required** (driver)
5. 2026-08-27 — Restore the model-significant `UserMessage` payload and place the cursor at the end;
   do not promise the pre-submit cursor or paste-burst placeholders that submission already clears.
   Why: the existing submission result does not retain those transient values. provisional
   (fresh-context review + driver)
6. 2026-08-27 — Track awaiting-acceptance, accepted, withdrawal-in-flight, and
   withdrawal-uncertain states per pending row. Why: repeated Up and late/transport-failed responses
   must never duplicate or falsely complete an edit. provisional (fresh-context review + driver)
7. 2026-08-27 — Submission transport/deserialization failure is acceptance-uncertain and therefore
   non-editable; definitive rejection follows the existing rejected-steer recovery path. Why: a
   request may have reached Core even when the TUI lacks an acknowledgement. provisional
   (fresh-context review + driver)
8. 2026-08-27 — Off-screen completions mutate the source thread's stored `ThreadInputState`, never
   the displayed widget. Why: request identity includes the originating thread and thread switches
   must not redirect state. provisional (fresh-context review + driver)
9. 2026-08-27 — A withdrawal-uncertain row remains non-editable across normal completion when no
   matching commit arrives; only a confirmed interruption may use the existing safe restore path.
   Why: normal completion alone cannot distinguish successful withdrawal from Core carrying the
   original into a follow-up sampling turn. provisional (fresh-context review + driver)
10. 2026-08-27 — The active `ChatWidget` is the canonical state owner when its thread matches;
    otherwise `ThreadEventStore.input_state` is canonical for a channel-backed thread and
    `agents_overview.input_states` is a synchronized navigation mirror. Why: off-screen responses
    must update every copy that can later restore the thread. provisional (fresh-context review +
    driver)
11. 2026-08-28 — The user ratified Decisions 1–10, the additive experimental API contract, and the
    named complete workspace test run. Why: the resumed implementation plan explicitly makes those
    decisions and authorization the governing assumptions. ratified (human)

## Work plan (ADF per unit)

### Task 1: Atomic Core withdrawal — high risk (public API dependency, ratified)

- **Outcome:** a thread can withdraw exactly one still-pending TUI user input by expected turn id
  and client user-message id; withdrawal and drain have one atomic winner.
- **Files:** modify `codex-rs/core/src/codex_thread.rs`,
  `codex-rs/core/src/lib.rs`, `codex-rs/core/src/session/input_queue.rs`, and
  `codex-rs/core/src/session/turn_input.rs`; test in `codex-rs/core/src/session/turn_input_tests.rs` and
  `codex-rs/core/tests/suite/pending_input.rs`.
- **Steps:** after the fresh PLAN gate, add the gated failing race tests and observe red; add a typed Core
  `WithdrawPendingInputResult` beside `CodexThread` and export it from `core/src/lib.rs`; expose
  `pub async fn withdraw_pending_input(&self, expected_turn_id: &str,
  client_user_message_id: &str) -> WithdrawPendingInputResult`. Its exhaustive variants are
  `Withdrawn { turn_id: String }`, `NoActiveTurn`, `ExpectedTurnMismatch { expected: String,
  actual: String }`, `NotPending { turn_id: String }`, and `AmbiguousClientId { turn_id: String }`.
  Exercise this public method directly in addition to any focused queue helper; remove in place
  under the existing
  `active_turn → turn_state` lock order. In integration tests, gate an MCP tool with
  `tokio::sync::Notify`: withdraw before releasing the tool for withdrawal-wins; for drain-wins,
  release it and await `StreamingSseServer::wait_for_request_count(2)` before withdrawing. Inspect
  the complete second model-bound request in both cases; use no sleeps. Re-run the focused tests.
- **Verification:** `just test -p codex-core <focused test filter>` and then
  `just test -p codex-core` pass.
- **Dependencies:** PLAN approval for Task 2's wire contract.

### Task 2: Experimental app-server contract — high risk, ratified

- **Outcome:** v2 exposes `turn/withdrawPendingInput` with `threadId`, `expectedTurnId`, and
  `clientUserMessageId`; success returns the accepted turn id and every rejection is explicit.
- **Files:** modify `codex-rs/app-server-protocol/src/protocol/v2/turn.rs`,
  `codex-rs/app-server-protocol/src/protocol/common.rs`,
  `codex-rs/app-server-protocol/src/export.rs`, and
  `codex-rs/app-server-protocol/src/schema_fixtures_tests.rs`,
  `codex-rs/app-server/src/message_processor.rs`,
  `codex-rs/app-server/src/request_processors.rs`, and
  `codex-rs/app-server/src/request_processors/turn_processor.rs`; create
  `codex-rs/app-server/src/request_processors/turn_pending_input.rs` for result/error mapping so the
  1,500-line turn processor gains only thin loading and dispatch plumbing; modify
  `codex-rs/app-server/README.md`, and generated schema/TypeScript fixtures; extend
  `codex-rs/app-server/tests/suite/v2/turn_steer.rs` and protocol schema tests.
- **Steps:** add the failing public JSON-RPC tests and observe red; add the
  exhaustive `ClientRequest` dispatch arm annotated
  `#[experimental("turn/withdrawPendingInput")]`; validate empty `threadId`, `expectedTurnId`, and
  `clientUserMessageId` in that order, then malformed and unloaded thread ids, with the exact
  messages and null data in the SPEC; preserve the existing direct-input-policy error; add
  structured `-32600` operational error data for `noActiveTurn`, `expectedTurnMismatch`,
  `notPending`, and `ambiguousClientUserMessageId`, including the exact message and nullable turn-id
  values in the SPEC table; prove exact success, drain-wins, every validation/lookup/policy and
  operational table row, duplicate-id, and unchanged ordinary repeated-id steer behavior;
  regenerate both schema modes and generated TypeScript. Add fixture/export assertions that the
  stable schema omits the method while the experimental schema includes the method, params,
  response, error object, and all error-reason variants. Register
  `TurnWithdrawPendingInputError` and `TurnWithdrawPendingInputErrorReason` in
  `EXPERIMENTAL_CLIENT_METHOD_DEPENDENCY_TYPES` because JSON-RPC error data is not reachable from
  the method's success response and therefore is not discovered as a dependency automatically.
- **Verification:** `just write-app-server-schema`, `just write-app-server-schema --experimental`,
  `just test -p codex-app-server-protocol`, and `just test -p codex-app-server` pass.
- **Dependencies:** fresh PLAN gate; Task 1 implementation.

### Task 3: Source-scoped TUI edit flow — medium risk

- **Outcome:** eligible plain Up withdraws the newest pending steer and restores its retained rich
  user-message payload after success without interrupting or redirecting across threads.
- **Files:** create `codex-rs/tui/src/app/pending_steer_edit.rs` to own source-thread dispatch and
  active/off-screen completion, `codex-rs/tui/src/app_server_session/pending_steer.rs` to own the
  typed RPC, and `codex-rs/tui/src/chatwidget/pending_steer.rs` to own pending identity and lifecycle
  outside the 758-line `user_messages.rs`; modify the module declarations in
  `codex-rs/tui/src/app.rs`, `codex-rs/tui/src/chatwidget.rs`,
  `codex-rs/tui/src/app_command.rs`, `codex-rs/tui/src/app_event.rs`,
  `codex-rs/tui/src/app/event_dispatch.rs`, `codex-rs/tui/src/app/thread_routing.rs`,
  `codex-rs/tui/src/app_server_session.rs`, `codex-rs/tui/src/chatwidget/interaction.rs`,
  `codex-rs/tui/src/chatwidget/input_submission.rs`,
  `codex-rs/tui/src/chatwidget/input_restore.rs`,
  `codex-rs/tui/src/chatwidget/input_queue.rs`,
  `codex-rs/tui/src/chatwidget/replay.rs`, `codex-rs/tui/src/chatwidget/user_messages.rs`, and
  `codex-rs/tui/src/bottom_pane/pending_input_preview.rs`; update the bottom-pane module docs if the
  composer state machine changes.
- **Tests:** extend `codex-rs/tui/src/chatwidget/tests/composer_submission.rs`,
  `codex-rs/tui/src/chatwidget/tests/helpers.rs`, and
  `codex-rs/tui/src/chatwidget/tests/review_mode.rs`; create
  `codex-rs/tui/src/app/tests/pending_steer_edit.rs` and register it in
  `codex-rs/tui/src/app/tests.rs`; the app test boots the real in-process app-server/Core and gates
  model SSE delivery to cover withdrawal-wins and drain-wins through the TUI request/result path.
  Update pending-input-preview `insta` snapshots.
- **Steps:** add failing key-routing, source-thread, delayed-restore, repeated-Up, late-response,
  transport-uncertainty, rich-message, identical-message, and failure tests and observe red;
  generate stable ids on `AppCommand::UserTurn` and carry them through both steer and start routing;
  keep central routing/session edits to thin argument forwarding and module declarations; implement
  a per-row state machine in `chatwidget/pending_steer.rs` for `AwaitingAcceptance`,
  `AcceptanceUncertain`, `Accepted { turn_id }`,
  `AwaitingCommitAfterStart`, `WithdrawalInFlight { turn_id, request_id }`, and
  `WithdrawalUncertain`. Bind steer success, including mismatch retry, to the response turn id;
  start fallback success waits for its ID-matched commit; definitive start/non-steerable/generic
  rejection uses the exact existing rejected-steer restore path; transport or deserialize failure
  becomes acceptance-uncertain; commit-before-response removes the row and makes the response a
  no-op. Implement every terminal action in the SPEC transition table. Reconcile commits and replay
  by id. The Up event clones an `AppServerRequestHandle` and spawns one bounded request task; its
  result returns as a correlated `AppEvent` so a thread switch can proceed while the RPC is in
  flight. Completion first mutates the active widget only when its thread matches and refreshes its
  channel snapshot; otherwise it mutates `ThreadEventStore.input_state` under that channel's lock.
  When `agents_overview.input_states` also has a copy, apply the same idempotent transition and
  overwrite the mirror from the canonical channel state if they diverged; when no channel state
  exists, the overview entry is the fallback owner. Restore only the exact matching row on success
  with its cursor at the end; update the preview hint and snapshots.
- **TDD cases:** before conversion, deep-compare the retained row's complete `UserMessage` and
  `UserMessageHistoryRecord::Override` with the submitted originals, covering local and remote
  images, text elements, mention bindings, and override text/elements. After success, compare the
  composer with `user_message_for_restore(original_message, &history_override)`: override text and
  elements form the editor representation, original images and mention bindings remain, pending
  pastes are empty, and the cursor equals the end of the restored ASCII text. Cover every submission
  transition and terminal action in the SPEC table, repeated Up, stale request ids, JSON-RPC rejection,
  transport/deserialization uncertainty, source thread switched before completion, turn-end,
  interrupt, replay, and identical display payloads with distinct client ids.
- **Verification:** focused test filters, then `just test -p codex-tui`; inspect every `.snap.new`
  and accept only the intended preview changes. Use the repository TUI harness for a live smoke of
  empty-composer Up, nonempty-composer Up, and switching threads before a withdrawal result; record
  the exact commands and observed evidence in the campaign State.
- **Dependencies:** Tasks 1 and 2.

### Task 4: Final gates and review — medium risk

- **Outcome:** all objective floors are green and a disinterested review reports no
  major-or-higher finding.
- **Steps:** run touched-crate suites and the authorized complete workspace `just test`; run scoped
  `just fix -p ...`, then
  `just fmt` without re-running tests; dispatch a fresh-context SPEC/BRIEF review and fix blocking
  findings for at most three rounds.
- **Verification:** named suite outputs, no pending snapshots, formatter/linter success, reviewer
  verdict below the `major` floor, and `git diff --check`.
- **Dependencies:** Tasks 1–3.

### Resource sketch

- Submission adds one UUID string to each TUI pending steer and its existing Core `TurnInput`.
- Plain Up adds one JSON-RPC round trip and one linear scan of the active turn's pending-input
  vector. No work is added to the sampling hot path beyond the existing drain lock.
- No durable storage, retry receipt, or model-context fragment is added. Each edit creates at most
  one bounded one-shot request task; it ends after one typed response or transport failure. A
  transport failure stays locally uncertain and non-editable until ordered lifecycle events
  reconcile it.

## Verification floors

- Focused Core pending-input test → withdrawal removes only the exact undelivered model input and
  a drain that wins the race causes withdrawal to fail closed.
- Focused app-server v2 test → the experimental wire request/response and client IDs preserve the
  atomic Core behavior without changing ordinary steer append semantics.
- Focused `codex-tui` tests → empty-composer plain Up restores the latest pending message without
  interrupting, duplication, or history regression; modal/popup and nonempty-composer cases retain
  their existing owners.
- In-process `codex-tui` integration test → the actual TUI client sends the experimental RPC through
  app-server to Core, with deterministic evidence for both withdrawal-wins and drain-wins.
- Live TUI smoke → contextual Up, nonempty-composer ownership, and source-thread isolation behave as
  the automated harness predicts; commands and observations are recorded before final review.
- `just test -p codex-tui` plus every other touched crate's `just test -p ...` → owning suites green.
- `just fmt` and scoped `just fix -p ...` → repository formatting and lints green.
- Review gate — harness first, briefed reviews. Severity floor `major` in the reviewer's own scale:
  findings at or above it block; below-floor-only findings do not. Maximum three review→fixup
  rounds. Deferred publish work is not a finding.

## Unblocking ladder

Investigate (two focused passes) → doctrine (Decisions here, BRIEF/SPEC Decisions, `doctrine.md`
in the loop-brief skill, memory) → `rl consult` with evidence + candidate approaches + spec
excerpts → provisional decision (dated entry above) → accumulate for the human (irreversible /
scope-changing / Boundary items only).

## In-session edit policy

The driver edits directly when the fix is finding-sized (≤ ~2 files, mechanical, fully
understood). After any in-session edit: run the owning gates and commit conventionally — the edit
lands in its unit's review scope; the driver never self-approves. Larger or design-shaped work goes
to a cook packet. Never mix in-session edits with an in-flight worker on the same files.

## Boundaries — NEVER

- Never push, open PRs, or merge — publish is the human's, per-artifact.
- Never touch live secrets or biometrics; never reroute around auth failures — surface and stop.
- Never edit only the TUI mirror of a pending steer while leaving the already accepted Core input
  unchanged.
- Never modify `CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR` or `CODEX_SANDBOX_ENV_VAR` behavior.

## Known pre-existing failures — do not chase (cited evidence only)

- None established.

## Terminal states & budget

- **done:** the contextual Up workflow edits the latest still-pending steer without interrupting;
  Core receives only the edited message; normal history/popup behavior is preserved; all named
  floors pass; a fresh-context review has no major-or-higher findings; nothing is pushed.
- **blocked:** numbered decision batch, each with evidence + a proposed answer; keep working
  independent items until only the batch remains.
- **budget:** hard cap `4` iterations for the campaign — or, earlier, three consecutive iterations
  without measurable movement on any checklist item → stop honestly with what was tried and why it
  cannot converge.
