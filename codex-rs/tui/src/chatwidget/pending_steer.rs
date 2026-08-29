//! Stable identity and retained display state for messages submitted into an active turn.

use std::collections::VecDeque;

use super::ChatWidget;
use super::ThreadInputState;
use super::UserMessage;
use super::UserMessageHistoryRecord;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PendingSteerLifecycle {
    AwaitingAcceptance,
    AcceptanceUncertain,
    Accepted { turn_id: String },
    AwaitingCommitAfterStart { turn_id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PendingSteer {
    pub(crate) client_user_message_id: String,
    pub(super) user_message: UserMessage,
    pub(super) history_record: UserMessageHistoryRecord,
    pub(crate) lifecycle: PendingSteerLifecycle,
}

impl PendingSteer {
    pub(crate) fn new(
        client_user_message_id: String,
        user_message: UserMessage,
        history_record: UserMessageHistoryRecord,
        lifecycle: PendingSteerLifecycle,
    ) -> Self {
        Self {
            client_user_message_id,
            user_message,
            history_record,
            lifecycle,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PendingSteerSubmissionOutcome {
    SteerAccepted { turn_id: String },
    StartAccepted { turn_id: String },
    DefinitivelyRejected,
    AcceptanceUncertain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingSteerSubmissionEffect {
    Noop,
    Updated,
    Rejected,
    BecameUncertain,
}

pub(super) fn take_pending_steer(
    pending_steers: &mut VecDeque<PendingSteer>,
    client_user_message_id: &str,
) -> Option<PendingSteer> {
    let index = pending_steers
        .iter()
        .position(|pending| pending.client_user_message_id == client_user_message_id)?;
    pending_steers.remove(index)
}

pub(super) fn reconcile_pending_steer_submission(
    pending_steers: &mut VecDeque<PendingSteer>,
    rejected_steers: &mut VecDeque<UserMessage>,
    rejected_history: &mut VecDeque<UserMessageHistoryRecord>,
    client_user_message_id: &str,
    outcome: PendingSteerSubmissionOutcome,
) -> PendingSteerSubmissionEffect {
    let Some(index) = pending_steers
        .iter()
        .position(|pending| pending.client_user_message_id == client_user_message_id)
    else {
        return PendingSteerSubmissionEffect::Noop;
    };
    if pending_steers[index].lifecycle != PendingSteerLifecycle::AwaitingAcceptance {
        return PendingSteerSubmissionEffect::Noop;
    }

    match outcome {
        PendingSteerSubmissionOutcome::SteerAccepted { turn_id } => {
            pending_steers[index].lifecycle = PendingSteerLifecycle::Accepted { turn_id };
            PendingSteerSubmissionEffect::Updated
        }
        PendingSteerSubmissionOutcome::StartAccepted { turn_id } => {
            pending_steers[index].lifecycle =
                PendingSteerLifecycle::AwaitingCommitAfterStart { turn_id };
            PendingSteerSubmissionEffect::Updated
        }
        PendingSteerSubmissionOutcome::DefinitivelyRejected => {
            let Some(pending) = pending_steers.remove(index) else {
                tracing::error!(index, "pending steer disappeared during reconciliation");
                return PendingSteerSubmissionEffect::Noop;
            };
            rejected_steers.push_back(pending.user_message);
            rejected_history.push_back(pending.history_record);
            PendingSteerSubmissionEffect::Rejected
        }
        PendingSteerSubmissionOutcome::AcceptanceUncertain => {
            pending_steers[index].lifecycle = PendingSteerLifecycle::AcceptanceUncertain;
            PendingSteerSubmissionEffect::BecameUncertain
        }
    }
}

impl ThreadInputState {
    pub(crate) fn reconcile_committed_pending_steer(&mut self, client_user_message_id: &str) {
        if let Some(committed) =
            take_pending_steer(&mut self.pending_steers, client_user_message_id)
        {
            self.committed_steers_for_replay.push_back(committed);
        }
    }

    pub(crate) fn reconcile_pending_steer_submission(
        &mut self,
        client_user_message_id: &str,
        outcome: PendingSteerSubmissionOutcome,
    ) -> PendingSteerSubmissionEffect {
        reconcile_pending_steer_submission(
            &mut self.pending_steers,
            &mut self.rejected_steers_queue,
            &mut self.rejected_steer_history_records,
            client_user_message_id,
            outcome,
        )
    }
}

impl ChatWidget {
    pub(crate) fn reconcile_pending_steer_submission(
        &mut self,
        client_user_message_id: &str,
        outcome: PendingSteerSubmissionOutcome,
    ) -> PendingSteerSubmissionEffect {
        let effect = reconcile_pending_steer_submission(
            &mut self.input_queue.pending_steers,
            &mut self.input_queue.rejected_steers_queue,
            &mut self.input_queue.rejected_steer_history_records,
            client_user_message_id,
            outcome,
        );
        if effect != PendingSteerSubmissionEffect::Noop {
            self.refresh_pending_input_preview();
        }
        effect
    }
}

#[cfg(test)]
#[path = "pending_steer_tests.rs"]
mod tests;
