use std::collections::VecDeque;
use std::path::PathBuf;

use codex_protocol::user_input::TextElement;
use pretty_assertions::assert_eq;

use super::*;
use crate::bottom_pane::LocalImageAttachment;
use crate::bottom_pane::MentionBinding;
use crate::chatwidget::UserMessage;
use crate::chatwidget::UserMessageHistoryOverride;
use crate::chatwidget::UserMessageHistoryRecord;

const CLIENT_ID: &str = "client-target";
const TURN_ID: &str = "turn-accepted";
const REQUEST_ID: &str = "withdraw-local-1";

#[test]
fn begin_requires_accepted_and_preserves_every_row_exactly() {
    let accepted = PendingSteerLifecycle::Accepted {
        turn_id: TURN_ID.to_string(),
    };
    let in_flight = in_flight_lifecycle();
    for (initial, expected_result, expected_lifecycle) in [
        (accepted, Some(TURN_ID.to_string()), in_flight.clone()),
        (
            PendingSteerLifecycle::AwaitingAcceptance,
            None,
            PendingSteerLifecycle::AwaitingAcceptance,
        ),
        (
            PendingSteerLifecycle::AcceptanceUncertain,
            None,
            PendingSteerLifecycle::AcceptanceUncertain,
        ),
        (
            PendingSteerLifecycle::AwaitingCommitAfterStart {
                turn_id: TURN_ID.to_string(),
            },
            None,
            PendingSteerLifecycle::AwaitingCommitAfterStart {
                turn_id: TURN_ID.to_string(),
            },
        ),
        (in_flight.clone(), None, in_flight),
        (uncertain_lifecycle(), None, uncertain_lifecycle()),
    ] {
        let mut pending = queue(initial);
        let result = begin_pending_steer_withdrawal(&mut pending, CLIENT_ID, REQUEST_ID);
        assert_eq!(
            (result, pending),
            (expected_result, queue(expected_lifecycle))
        );
    }
}

#[test]
fn each_correlated_outcome_has_an_exact_effect_and_queue() {
    let in_flight_row = target(in_flight_lifecycle());
    for (outcome, expected_effect, expected_queue) in [
        (
            PendingSteerWithdrawalOutcome::Withdrawn,
            PendingSteerWithdrawalEffect::Withdrawn(Box::new(in_flight_row)),
            unrelated_queue(),
        ),
        (
            PendingSteerWithdrawalOutcome::Rejected,
            PendingSteerWithdrawalEffect::Rejected,
            queue(PendingSteerLifecycle::Accepted {
                turn_id: TURN_ID.to_string(),
            }),
        ),
        (
            PendingSteerWithdrawalOutcome::Uncertain,
            PendingSteerWithdrawalEffect::BecameUncertain,
            queue(uncertain_lifecycle()),
        ),
    ] {
        let mut pending = queue(in_flight_lifecycle());
        let effect = reconcile_pending_steer_withdrawal(
            &mut pending,
            CLIENT_ID,
            TURN_ID,
            REQUEST_ID,
            outcome,
        );
        assert_eq!((effect, pending), (expected_effect, expected_queue));
    }
}

#[test]
fn repeated_stale_and_post_commit_results_are_exact_noops() {
    let in_flight = queue(in_flight_lifecycle());
    let mut repeated = in_flight.clone();
    assert_eq!(
        (
            begin_pending_steer_withdrawal(&mut repeated, CLIENT_ID, "second-request"),
            repeated,
        ),
        (None, in_flight.clone())
    );

    for (client_id, turn_id, request_id) in [
        ("other-client", TURN_ID, REQUEST_ID),
        (CLIENT_ID, "other-turn", REQUEST_ID),
        (CLIENT_ID, TURN_ID, "other-request"),
    ] {
        let mut pending = in_flight.clone();
        let effect = reconcile_pending_steer_withdrawal(
            &mut pending,
            client_id,
            turn_id,
            request_id,
            PendingSteerWithdrawalOutcome::Withdrawn,
        );
        assert_eq!(
            (effect, pending),
            (PendingSteerWithdrawalEffect::Noop, in_flight.clone())
        );
    }

    for lifecycle in [
        PendingSteerLifecycle::Accepted {
            turn_id: TURN_ID.to_string(),
        },
        uncertain_lifecycle(),
    ] {
        let original = queue(lifecycle);
        for outcome in [
            PendingSteerWithdrawalOutcome::Withdrawn,
            PendingSteerWithdrawalOutcome::Rejected,
            PendingSteerWithdrawalOutcome::Uncertain,
        ] {
            let mut pending = original.clone();
            let effect = reconcile_pending_steer_withdrawal(
                &mut pending,
                CLIENT_ID,
                TURN_ID,
                REQUEST_ID,
                outcome,
            );
            assert_eq!(
                (effect, pending),
                (PendingSteerWithdrawalEffect::Noop, original.clone())
            );
        }
    }

    for outcome in [
        PendingSteerWithdrawalOutcome::Withdrawn,
        PendingSteerWithdrawalOutcome::Rejected,
        PendingSteerWithdrawalOutcome::Uncertain,
    ] {
        let mut committed = unrelated_queue();
        let effect = reconcile_pending_steer_withdrawal(
            &mut committed,
            CLIENT_ID,
            TURN_ID,
            REQUEST_ID,
            outcome,
        );
        assert_eq!(
            (effect, committed),
            (PendingSteerWithdrawalEffect::Noop, unrelated_queue())
        );
    }
}

fn queue(lifecycle: PendingSteerLifecycle) -> VecDeque<PendingSteer> {
    let mut pending = unrelated_queue();
    pending.push_back(target(lifecycle));
    pending
}

fn unrelated_queue() -> VecDeque<PendingSteer> {
    VecDeque::from([PendingSteer {
        client_user_message_id: "client-unrelated".to_string(),
        user_message: UserMessage::from("unrelated"),
        history_record: UserMessageHistoryRecord::UserMessageText,
        lifecycle: PendingSteerLifecycle::AwaitingAcceptance,
    }])
}

fn target(lifecycle: PendingSteerLifecycle) -> PendingSteer {
    PendingSteer {
        client_user_message_id: CLIENT_ID.to_string(),
        user_message: UserMessage {
            text: "$skill inspect [Image #1]".to_string(),
            local_images: vec![LocalImageAttachment {
                placeholder: "[Image #1]".to_string(),
                path: PathBuf::from("/tmp/pending-local.png"),
            }],
            remote_image_urls: vec!["https://example.test/pending.png".to_string()],
            text_elements: vec![TextElement::new((0..6).into(), Some("$skill".to_string()))],
            mention_bindings: vec![MentionBinding {
                sigil: '$',
                mention: "skill".to_string(),
                path: "/tmp/skills/skill/SKILL.md".to_string(),
            }],
        },
        history_record: UserMessageHistoryRecord::Override(UserMessageHistoryOverride {
            text: "history override".to_string(),
            text_elements: vec![TextElement::new((0..7).into(), /*placeholder*/ None)],
        }),
        lifecycle,
    }
}

fn in_flight_lifecycle() -> PendingSteerLifecycle {
    PendingSteerLifecycle::WithdrawalInFlight {
        accepted_turn_id: TURN_ID.to_string(),
        request_id: REQUEST_ID.to_string(),
    }
}

fn uncertain_lifecycle() -> PendingSteerLifecycle {
    PendingSteerLifecycle::WithdrawalUncertain {
        accepted_turn_id: TURN_ID.to_string(),
        request_id: REQUEST_ID.to_string(),
    }
}
