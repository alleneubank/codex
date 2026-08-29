use std::collections::VecDeque;

use pretty_assertions::assert_eq;

use super::*;
use crate::chatwidget::UserMessageHistoryOverride;

const TARGET_ID: &str = "client-target";

#[derive(Debug, Clone, Default, PartialEq)]
struct Queues {
    pending: VecDeque<PendingSteer>,
    committed: VecDeque<PendingSteer>,
    rejected: VecDeque<UserMessage>,
    rejected_history: VecDeque<UserMessageHistoryRecord>,
}

impl Queues {
    fn reconcile(
        &mut self,
        client_user_message_id: &str,
        outcome: PendingSteerSubmissionOutcome,
    ) -> PendingSteerSubmissionEffect {
        reconcile_pending_steer_submission(
            &mut self.pending,
            &mut self.rejected,
            &mut self.rejected_history,
            client_user_message_id,
            outcome,
        )
    }

    fn commit(&mut self, client_user_message_id: &str) {
        let committed = take_pending_steer(&mut self.pending, client_user_message_id)
            .expect("the committed client ID is pending in this fixture");
        self.committed.push_back(committed);
    }
}

#[test]
fn awaiting_acceptance_reconciles_every_submission_outcome() {
    let cases = [
        (
            "steer accepted",
            PendingSteerSubmissionOutcome::SteerAccepted {
                turn_id: "turn-steer-response".to_string(),
            },
            PendingSteerSubmissionEffect::Updated,
            queues_with_target(PendingSteerLifecycle::Accepted {
                turn_id: "turn-steer-response".to_string(),
            }),
        ),
        (
            "start accepted",
            PendingSteerSubmissionOutcome::StartAccepted {
                turn_id: "turn-start-response".to_string(),
            },
            PendingSteerSubmissionEffect::Updated,
            queues_with_target(PendingSteerLifecycle::AwaitingCommitAfterStart {
                turn_id: "turn-start-response".to_string(),
            }),
        ),
        (
            "definitively rejected",
            PendingSteerSubmissionOutcome::DefinitivelyRejected,
            PendingSteerSubmissionEffect::Rejected,
            queues_after_target_rejection(),
        ),
        (
            "acceptance uncertain",
            PendingSteerSubmissionOutcome::AcceptanceUncertain,
            PendingSteerSubmissionEffect::BecameUncertain,
            queues_with_target(PendingSteerLifecycle::AcceptanceUncertain),
        ),
    ];

    for (name, outcome, expected_effect, expected_queues) in cases {
        let mut queues = queues_with_target(PendingSteerLifecycle::AwaitingAcceptance);

        let effect = queues.reconcile(TARGET_ID, outcome);

        assert_eq!(
            (name, effect, queues),
            (name, expected_effect, expected_queues)
        );
    }
}

#[test]
fn settled_states_ignore_every_later_submission_outcome() {
    let settled_states = [
        (
            "accepted",
            PendingSteerLifecycle::Accepted {
                turn_id: "turn-original-steer".to_string(),
            },
        ),
        (
            "awaiting commit after start",
            PendingSteerLifecycle::AwaitingCommitAfterStart {
                turn_id: "turn-original-start".to_string(),
            },
        ),
    ];

    for (state_name, lifecycle) in settled_states {
        for (outcome_name, outcome) in submission_outcomes() {
            let expected_queues = queues_with_target(lifecycle.clone());
            let mut queues = expected_queues.clone();

            let effect = queues.reconcile(TARGET_ID, outcome);

            assert_eq!(
                (state_name, outcome_name, effect, queues),
                (
                    state_name,
                    outcome_name,
                    PendingSteerSubmissionEffect::Noop,
                    expected_queues,
                )
            );
        }
    }
}

#[test]
fn missing_or_already_committed_rows_ignore_every_late_outcome() {
    let missing_queues = queues_with_target(PendingSteerLifecycle::AwaitingAcceptance);
    let mut committed_queues = missing_queues.clone();
    committed_queues.commit(TARGET_ID);
    assert_eq!(committed_queues, queues_after_target_commit());

    for (scenario_name, client_user_message_id, initial_queues) in [
        ("missing ID", "client-missing", missing_queues),
        ("commit before response", TARGET_ID, committed_queues),
    ] {
        for (outcome_name, outcome) in submission_outcomes() {
            let expected_queues = initial_queues.clone();
            let mut queues = initial_queues.clone();

            let effect = queues.reconcile(client_user_message_id, outcome);

            assert_eq!(
                (scenario_name, outcome_name, effect, queues),
                (
                    scenario_name,
                    outcome_name,
                    PendingSteerSubmissionEffect::Noop,
                    expected_queues,
                )
            );
        }
    }
}

fn submission_outcomes() -> [(&'static str, PendingSteerSubmissionOutcome); 4] {
    [
        (
            "steer accepted",
            PendingSteerSubmissionOutcome::SteerAccepted {
                turn_id: "turn-late-steer".to_string(),
            },
        ),
        (
            "start accepted",
            PendingSteerSubmissionOutcome::StartAccepted {
                turn_id: "turn-late-start".to_string(),
            },
        ),
        (
            "definitively rejected",
            PendingSteerSubmissionOutcome::DefinitivelyRejected,
        ),
        (
            "acceptance uncertain",
            PendingSteerSubmissionOutcome::AcceptanceUncertain,
        ),
    ]
}

fn queues_with_target(lifecycle: PendingSteerLifecycle) -> Queues {
    Queues {
        pending: VecDeque::from([
            pending_steer(
                "unrelated",
                "client-unrelated",
                UserMessageHistoryRecord::UserMessageText,
                PendingSteerLifecycle::AwaitingAcceptance,
            ),
            pending_steer("target", TARGET_ID, target_history_record(), lifecycle),
        ]),
        rejected: VecDeque::from([UserMessage::from("previously rejected")]),
        rejected_history: VecDeque::from([UserMessageHistoryRecord::UserMessageText]),
        ..Default::default()
    }
}

fn queues_after_target_rejection() -> Queues {
    Queues {
        pending: VecDeque::from([pending_steer(
            "unrelated",
            "client-unrelated",
            UserMessageHistoryRecord::UserMessageText,
            PendingSteerLifecycle::AwaitingAcceptance,
        )]),
        rejected: VecDeque::from([
            UserMessage::from("previously rejected"),
            UserMessage::from("target"),
        ]),
        rejected_history: VecDeque::from([
            UserMessageHistoryRecord::UserMessageText,
            target_history_record(),
        ]),
        ..Default::default()
    }
}

fn queues_after_target_commit() -> Queues {
    Queues {
        pending: VecDeque::from([pending_steer(
            "unrelated",
            "client-unrelated",
            UserMessageHistoryRecord::UserMessageText,
            PendingSteerLifecycle::AwaitingAcceptance,
        )]),
        committed: VecDeque::from([pending_steer(
            "target",
            TARGET_ID,
            target_history_record(),
            PendingSteerLifecycle::AwaitingAcceptance,
        )]),
        rejected: VecDeque::from([UserMessage::from("previously rejected")]),
        rejected_history: VecDeque::from([UserMessageHistoryRecord::UserMessageText]),
    }
}

fn pending_steer(
    text: &str,
    client_user_message_id: &str,
    history_record: UserMessageHistoryRecord,
    lifecycle: PendingSteerLifecycle,
) -> PendingSteer {
    PendingSteer {
        client_user_message_id: client_user_message_id.to_string(),
        user_message: UserMessage::from(text),
        history_record,
        lifecycle,
    }
}

fn target_history_record() -> UserMessageHistoryRecord {
    UserMessageHistoryRecord::Override(UserMessageHistoryOverride {
        text: "target history override".to_string(),
        text_elements: Vec::new(),
    })
}
