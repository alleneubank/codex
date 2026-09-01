# Core Sampling Retry Policy

Model capacity failures can outlive the short transport retry window without being permanent.
Ordinary sampling turns therefore own a bounded, user-visible capacity retry lifecycle, while
shared error classification remains terminal so other callers do not accidentally inherit long
backoffs.

## Domain model

- A capacity failure is `server_is_overloaded` or `slow_down` returned by an HTTP 503, an SSE
  failure, or a WebSocket error.
- An ordinary sampling turn is a model sampling turn whose session source is not a Guardian
  reviewer.
- The transport retry budget handles short HTTP and connection failures. The stream retry budget
  handles ordinary retryable response-stream failures. The capacity retry budget is independent
  of both.
- A retrying capacity failure is exposed as a `ServerOverloaded` stream error. Exhaustion is
  exposed as one terminal `ServerOverloaded` error.

## Requirements

- **REQ-CAP-001** — Ordinary sampling turns retry capacity failures received through HTTP, SSE,
  and WebSocket transports without changing the turn ID between attempts.
- **REQ-CAP-002** — Capacity retries have exactly three attempts after the initial request. Their
  base delays are 30, 120, and 300 seconds, each with positive jitter that never shortens the base
  delay. A server-provided `Retry-After` value does not replace or shorten this schedule.
- **REQ-CAP-003** — Capacity retries do not consume or reset the generic stream retry budget, and
  generic stream retries do not consume or reset the capacity budget.
- **REQ-CAP-004** — When the capacity budget is exhausted, the turn emits exactly one terminal
  `ServerOverloaded` error whose message ends with `Please try again later.`
- **REQ-CAP-005** — Interrupting or cancelling a turn during a capacity backoff cancels the wait
  and does not issue another model request.
- **REQ-CAP-006** — Guardian reviewer sampling keeps capacity failures terminal at the sampling
  turn boundary. Remote compaction and other non-sampling callers do not inherit the capacity
  policy.
- **REQ-CAP-007** — When an ordinary sampling turn owns capacity retries, a capacity-coded HTTP
  503 bypasses the short transport retry layer. Other HTTP 5xx responses retain their existing
  transport retry behavior.
- **REQ-CAP-008** — Every retrying capacity failure emits an intermediate `ServerOverloaded`
  stream error that app-server clients expose with `willRetry: true`; the TUI keeps the turn
  running and renders the retry state.
- **REQ-CAP-009** — Each capacity retry attempt emits exactly one retry telemetry event with the
  sampling operation, selected delay, and one-based attempt number.

## Invariants

- `CodexErr::is_retryable()` continues to classify `ServerOverloaded` as terminal.
- At most one layer owns a capacity retry for a request attempt.
- Capacity retries are bounded and cancellation-aware.
- A successful retry resumes the same logical turn and emits no terminal capacity error.
- No retry path duplicates telemetry for one attempt.

## Non-goals

- Making every `ServerOverloaded` error globally retryable.
- Configuring the capacity schedule or attempt count through `config.toml`.
- Retrying Guardian reviewer turns through the long capacity schedule.
- Changing rate-limit, generic 5xx, remote-compaction, or connection-retry policy.

## Decisions

- **ratified — 2026-09-01:** Capacity retry ownership belongs to ordinary sampling turns rather
  than the shared error type or every Responses API caller.
- **ratified — 2026-09-01:** The fork preserves the original bounded 30/120/300-second schedule
  with positive jitter and ignores shorter server advice for capacity failures.

## Risk tags

- **Public behavior:** app-server clients observe retrying overload notifications where ordinary
  sampling previously failed immediately.
- **Operational latency:** a capacity-bound turn may remain active through three long,
  cancellation-aware backoffs.

## Acceptance criteria

- [x] HTTP, SSE, and WebSocket capacity failures recover within the capacity budget.
- [x] Capacity and generic stream budgets remain independent.
- [x] Capacity-coded HTTP 503s are not retried by both transport and turn layers.
- [x] Non-capacity 503 transport behavior is unchanged.
- [x] Capacity backoff cancellation prevents another request.
- [x] Exhaustion emits three retry states and exactly one terminal error.
- [x] Guardian reviewer overload remains terminal at the sampling boundary.
- [x] Retry telemetry is emitted exactly once per capacity retry attempt.
- [x] TUI snapshot coverage shows overload as retrying without finalizing the active turn.

## Traceability

- **REQ-CAP-001, REQ-CAP-002:** `responses_http_capacity_retry_uses_turn_backoff_despite_retry_after`,
  `sse_overload_with_retry_after_retries`, `sse_overload_without_retry_after_retries`,
  `websocket_overload_with_nested_retry_after_retries`, and
  `websocket_overload_without_retry_after_retries` cover all three transports and server advice.
- **REQ-CAP-003:** `capacity_retries_use_a_separate_budget_on_the_same_turn` observes a generic
  retry and a capacity retry on one unchanged turn ID.
- **REQ-CAP-004, REQ-CAP-009:**
  `capacity_exhaustion_emits_three_retries_and_one_terminal_error` and
  `responses_http_capacity_exhausts_turn_retries` cover exhaustion, terminal cardinality, and
  one telemetry event per selected delay.
- **REQ-CAP-005:** `interrupting_capacity_backoff_prevents_another_request` interrupts the wait
  and observes no follow-up request.
- **REQ-CAP-006:** `guardian_review_retries_transient_session_failure_then_approves` observes
  terminal sampling attempts with distinct turn IDs, while
  `compact_v2_overload_without_retry_after_exhausts_request_retries` keeps remote compaction on
  its existing terminal path.
- **REQ-CAP-007:** `server_overload_transport_detection_is_narrow`,
  `responses_http_capacity_retry_uses_turn_backoff_despite_retry_after`, and
  `http_retry_backoff_exhausts_attempts` cover capacity ownership and unchanged generic HTTP
  retry behavior.
- **REQ-CAP-008:** `live_app_server_server_overloaded_retry_keeps_turn_running` snapshots the
  app-server overload as retrying while the TUI task and composer remain active.
