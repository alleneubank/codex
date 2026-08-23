# TUI Permission-Mode Switching

The TUI permission switcher lets a user move between the built-in permission modes without
opening `/permissions`. The switcher must expose every built-in mode allowed by managed
requirements while preserving the safety UI and session scoping of the equivalent picker flow.

## Domain model

- A built-in approval preset pairs an approval policy, an active permission-profile id, and a
  concrete permission profile.
- A permission shortcut selects the next or previous allowed preset for the displayed thread.
- Full Access is the `:danger-full-access` profile with `Never` approvals. It is applied only after
  the user accepts the full-access confirmation.
- Confirmed shortcut changes update the active thread through app-server and remain in-memory;
  they do not write `config.toml`.

## Requirements

- **REQ-PERM-SWITCH-001**: Next and previous permission shortcuts include every allowed built-in
  mode, including Full Access.
- **REQ-PERM-SWITCH-002**: Selecting Full Access from a shortcut opens the full-access confirmation
  before changing the active permission profile or approval policy.
- **REQ-PERM-SWITCH-003**: Accepting the confirmation applies Full Access only to the thread that
  originated the shortcut and does not persist the selection to `config.toml`.
- **REQ-PERM-SWITCH-004**: Cancelling the confirmation leaves permissions unchanged and does not
  prevent a later permission shortcut.
- **REQ-PERM-SWITCH-005**: Managed approval, reviewer, and permission-profile requirements remove
  forbidden modes from the shortcut cycle.

## Invariants

- No shortcut bypasses the Full Access confirmation.
- A stale shortcut event cannot change a thread other than its originating displayed thread.
- Shortcut application is session-scoped and emits one success or failure history cell.

## Non-goals

- The switcher does not persist a new default permission profile.
- The switcher does not add custom named profiles to the built-in shortcut cycle.
- The change does not alter Full Access sandbox semantics or managed requirement evaluation.

## Decisions

- 2026-08-23 — ratified: Full Access is part of the permission shortcut cycle.
- 2026-08-23 — provisional: The existing Full Access confirmation remains the authority boundary,
  and acceptance reuses the shortcut's source-thread app-server update path.

## Risk

This changes a security boundary because it makes elevated authority reachable from a keyboard
shortcut. The confirmation and managed-requirement checks are mandatory acceptance floors.

## Acceptance criteria

- [x] Forward and reverse cycling can select allowed Full Access.
- [x] Full Access is not applied before confirmation.
- [x] Accept applies session-only Full Access to the originating thread.
- [x] Cancel leaves permissions unchanged and the switcher remains usable.
- [x] Disallowed Full Access is skipped.

## Test traceability

- `REQ-PERM-SWITCH-001`, `REQ-PERM-SWITCH-002`, `REQ-PERM-SWITCH-004`, and
  `REQ-PERM-SWITCH-005`: `src/chatwidget/tests/permission_shortcuts_tests.rs`
- `REQ-PERM-SWITCH-003`: `src/app/tests/permission_shortcuts_tests.rs`
