---
loop: 1
id: mission-slash-remap
objective: Make /mission a thin consumer of missionctl's bounded projections (context, check, mission, inspect) with no portfolio or prompt views.
status: done
phase: BOUNDARY
iteration: 1
iteration_budget: 3
updated_at: 2026-08-29T17:20:00Z
mission:
  id: mission-control-arc
  source:
    repository: https://github.com/alleneubank/missionctl.git
    ref: feat/typed-mission-control
    path: .mission/mission.yaml
targets:
  mission: [CONSUMERS-001]
gates:
  - id: mission-tests
    run: cargo test -p codex-tui --lib -- mission_command_ mission_slash_command
    green: every /mission unit test passes
    state: green
  - id: popup-snapshot
    run: cargo test -p codex-tui --lib -- command_popup
    green: the slash-command popup snapshot matches
    state: green
units:
  - id: U1
    title: "/mission maps to missionctl context|check|mission|inspect; usage, description, snapshot, and tests follow"
    state: done
decisions: []
blockers: []
boundary:
  - publish
  - merge-tracked-ref
  - force-push-pr-branch
---

# Loop: /mission remap — `feat/mission-command`

## State

- Local `feat/mission-command` is rebased on the fork `main` (ahead 346 / behind 26 of origin); updating PR #1 needs a force-push, which is the human's call.
- The full `codex-tui` lib run under the `mission` filter aborts in an unrelated `…permissions…` test (stack overflow in debug); the exact-name gates above are the campaign's verifier.
