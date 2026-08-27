# Lifecycle Hooks

Codex lifecycle hooks let external automation observe or influence named points in a
thread without confusing user-attention telemetry with approval policy. In particular,
an interactive question is not a permission request: `Notification` reports that Codex
is waiting for input, while `PermissionRequest` remains reserved for tool and sandbox
approval decisions.

## Domain model

- A hook event name selects matcher groups from the active configuration layers and
  enabled plugins.
- A `Notification` matcher is evaluated against `notification_type`.
- A user-attention notification has an open type (`user_input_request`,
  `elicitation_dialog`, `elicitation_url_dialog`, or `plan_implementation_request`) and
  a corresponding completion type (`user_input_complete`, `elicitation_complete`, or
  `plan_implementation_complete`).
- Native questions, user-visible MCP elicitations, and the post-plan implementation
  decision own the notification pair for the lifetime of the interactive wait.
  Policy-denied, automatically resolved, or programmatically reviewed MCP elicitations
  do not create an interactive wait and do not emit the pair.
- Notification handlers receive the common command-hook fields plus
  `notification_type`, a bounded generic `message`, and an optional generic `title`.
  Their output has no control effect on the prompt or its response.

## Requirements

- **REQ-HOOK-NOTIFY-001** — `Notification` is a distinct hook event. Its matcher filters
  on the exact `notification_type`; an omitted, empty, or `*` matcher selects every
  notification type under the existing matcher rules.
- **REQ-HOOK-NOTIFY-002** — Codex emits `user_input_request` immediately after exposing
  every native `request_user_input` prompt, whether blocking or non-blocking, and emits
  exactly one `user_input_complete` when that wait is answered, auto-resolved,
  dismissed, interrupted, or dropped.
- **REQ-HOOK-NOTIFY-003** — Codex emits `elicitation_dialog` for user-visible MCP form
  and OpenAI-form elicitations and `elicitation_url_dialog` for user-visible MCP URL
  elicitations. Each open notification has exactly one `elicitation_complete` when the
  wait ends.
- **REQ-HOOK-NOTIFY-004** — MCP elicitations resolved without displaying an interactive
  request, including automatic policy outcomes and programmatic reviewer responses,
  emit no user-attention notification.
- **REQ-HOOK-NOTIFY-005** — Notification handlers are observers. Synchronous and
  asynchronous handlers may perform side effects and report execution failures, but
  their exit status, stdout, or structured output cannot answer, deny, suppress, or
  otherwise change delivery or resolution of the interactive request.
- **REQ-HOOK-NOTIFY-006** — Notification hook payloads never include question text,
  answer data, form contents, elicitation URLs, or other request-specific content.
  `message` and `title`, when present, are fixed generic strings bounded by their
  compile-time constants.
- **REQ-HOOK-NOTIFY-007** — A notification serialization or handler failure is fail-open:
  Codex reports the hook failure through the normal hook lifecycle surfaces and still
  exposes or completes the interactive request.
- **REQ-HOOK-NOTIFY-008** — `Notification` is discoverable through configuration,
  managed configuration, plugin declarations, `hooks/list`, telemetry labels, the
  generated command-input schema, and app-server protocol schemas.
- **REQ-HOOK-NOTIFY-009** — Before the TUI exposes the post-plan “Implement this plan?”
  decision, it starts `plan_implementation_request` against the completed turn. Every
  selection and every dismiss or cancel path emits exactly one
  `plan_implementation_complete`. A signaling failure is observable and fail-open: the
  decision remains available.
- **REQ-HOOK-NOTIFY-010** — Experimental app-server methods
  `thread/userAttention/start` and `thread/userAttention/complete` correlate a lifecycle
  by connection, thread, and opaque attention ID. Active duplicate starts fail,
  completion is idempotent, and connection close or thread teardown completes owned
  lifecycles exactly once.
- **REQ-HOOK-NOTIFY-011** — Post-turn attention uses a bounded, thread-scoped snapshot
  of hook context from the named completed turn. It does not retain an expired turn
  context or consume the unrelated elicitation counter.

## Invariants

- `PermissionRequest` is emitted only for actual tool or sandbox approval decisions.
- Every emitted user-attention open notification has one completion notification.
- Completion payloads contain no response data.
- Hook execution cannot prevent prompt delivery or change the interactive response.
- Correlation IDs and plan contents remain in the app client and never enter hook stdin.

## Non-goals

- Desktop notification timing, focus detection, and Claude's idle-delay policy are not
  reproduced.
- `permission_prompt`, `idle_prompt`, and unrelated notification types are not emitted.
- Notification hooks do not gain a decision or response API.
- TUI rendering and snapshot output are unchanged.

## Decisions

- **ratified — 2026-08-26:** User-attention lifecycle telemetry fires immediately at the
  interactive-request boundary rather than after a focus or idle delay.
- **ratified — 2026-08-26:** The command input follows Claude's `Notification` field
  names and Claude-compatible common fields.
- **ratified — 2026-08-26:** Generic payload text is used so external hook commands do
  not receive secrets or user-authored content through stdin or process arguments.
- **ratified — 2026-08-30:** Post-plan attention is a correlated post-turn lifecycle;
  it uses additive notification names and experimental app-server methods rather than
  retaining the completed turn context or overloading elicitation state.

## Risk tags

- **Public API contract:** `HookEventName`, hook configuration, app-server schemas, and
  generated SDK artifacts gain an additive enum variant and input schema.
- **Lifecycle correctness:** missing or duplicate completion events can leave external
  command centers in a false attention state.

## Waivers

- **2026-08-30 — change size:** The correlated lifecycle crosses the hook contract,
  core ownership, experimental app-server API, TUI boundary, generated fixtures, and
  their integration harnesses. It remains one rewrite of the existing upstream-bound
  hook feature commit so the public contract and its verifier land atomically.

## Acceptance criteria

- [x] Native blocking, non-blocking, answered, auto-resolved, interrupted, and dropped
      waits satisfy the paired ordering contract.
- [x] User-visible MCP form, OpenAI-form, and URL elicitations satisfy the paired
      ordering contract.
- [x] Automatically or programmatically resolved MCP elicitations remain silent.
- [x] Post-plan implementation choices and Escape satisfy start-before-display and
      exact-once completion, including disconnect and thread-teardown cleanup.
- [x] Matcher selection, synchronous and asynchronous execution, observer failure
      behavior, discovery, `hooks/list`, serialization, telemetry labels, and generated
      schemas are covered by passing tests.
- [x] Targeted `codex-hooks`, `codex-config`, `codex-core`,
      `codex-app-server-protocol`, and `codex-app-server` verifiers pass.

## Test traceability

- REQ-HOOK-NOTIFY-001 and REQ-HOOK-NOTIFY-005 through REQ-HOOK-NOTIFY-008 map to
  `notification_tests`, hook schema tests, `hooks_list`, config tests, and hook telemetry
  tests.
- REQ-HOOK-NOTIFY-002 maps to the `request_user_input` integration suite.
- REQ-HOOK-NOTIFY-003 and REQ-HOOK-NOTIFY-004 map to `elicitation_tests`.
- REQ-HOOK-NOTIFY-009 maps to TUI `plan_implementation_*` tests and the core
  `user_attention` integration test.
- REQ-HOOK-NOTIFY-010 maps to app-server v2 `user_attention` tests covering concurrent
  IDs, duplicate rejection, completion retries, invalid turns, disconnect, and archive
  cleanup.
- REQ-HOOK-NOTIFY-011 maps to the core `user_attention` integration test and completed
  turn context capture in the core lifecycle harness.
