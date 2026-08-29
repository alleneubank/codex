//! Atomic local state transitions for editing an accepted pending steer.

use std::collections::VecDeque;

use super::ChatWidget;
use super::PendingSteer;
use super::PendingSteerLifecycle;
use super::ThreadInputState;
use super::pending_steer::take_pending_steer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingSteerWithdrawalOutcome {
    Withdrawn,
    Rejected,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PendingSteerWithdrawalEffect {
    Noop,
    Withdrawn(Box<PendingSteer>),
    Rejected,
    BecameUncertain,
}

pub(super) fn begin_pending_steer_withdrawal(
    pending_steers: &mut VecDeque<PendingSteer>,
    client_user_message_id: &str,
    request_id: &str,
) -> Option<String> {
    let pending = pending_steers
        .iter_mut()
        .find(|pending| pending.client_user_message_id == client_user_message_id)?;
    let PendingSteerLifecycle::Accepted { turn_id } = &pending.lifecycle else {
        return None;
    };
    let accepted_turn_id = turn_id.clone();
    pending.lifecycle = PendingSteerLifecycle::WithdrawalInFlight {
        accepted_turn_id: accepted_turn_id.clone(),
        request_id: request_id.to_string(),
    };
    Some(accepted_turn_id)
}

pub(super) fn reconcile_pending_steer_withdrawal(
    pending_steers: &mut VecDeque<PendingSteer>,
    client_user_message_id: &str,
    accepted_turn_id: &str,
    request_id: &str,
    outcome: PendingSteerWithdrawalOutcome,
) -> PendingSteerWithdrawalEffect {
    let Some(pending) = pending_steers
        .iter_mut()
        .find(|pending| pending.client_user_message_id == client_user_message_id)
    else {
        return PendingSteerWithdrawalEffect::Noop;
    };
    if pending.lifecycle
        != (PendingSteerLifecycle::WithdrawalInFlight {
            accepted_turn_id: accepted_turn_id.to_string(),
            request_id: request_id.to_string(),
        })
    {
        return PendingSteerWithdrawalEffect::Noop;
    }

    match outcome {
        PendingSteerWithdrawalOutcome::Withdrawn => {
            take_pending_steer(pending_steers, client_user_message_id)
                .map(Box::new)
                .map(PendingSteerWithdrawalEffect::Withdrawn)
                .unwrap_or(PendingSteerWithdrawalEffect::Noop)
        }
        PendingSteerWithdrawalOutcome::Rejected => {
            pending.lifecycle = PendingSteerLifecycle::Accepted {
                turn_id: accepted_turn_id.to_string(),
            };
            PendingSteerWithdrawalEffect::Rejected
        }
        PendingSteerWithdrawalOutcome::Uncertain => {
            pending.lifecycle = PendingSteerLifecycle::WithdrawalUncertain {
                accepted_turn_id: accepted_turn_id.to_string(),
                request_id: request_id.to_string(),
            };
            PendingSteerWithdrawalEffect::BecameUncertain
        }
    }
}

impl ThreadInputState {
    pub(crate) fn begin_pending_steer_withdrawal(
        &mut self,
        client_user_message_id: &str,
        request_id: &str,
    ) -> Option<String> {
        begin_pending_steer_withdrawal(&mut self.pending_steers, client_user_message_id, request_id)
    }

    pub(crate) fn reconcile_pending_steer_withdrawal(
        &mut self,
        client_user_message_id: &str,
        accepted_turn_id: &str,
        request_id: &str,
        outcome: PendingSteerWithdrawalOutcome,
    ) -> PendingSteerWithdrawalEffect {
        reconcile_pending_steer_withdrawal(
            &mut self.pending_steers,
            client_user_message_id,
            accepted_turn_id,
            request_id,
            outcome,
        )
    }
}

impl ChatWidget {
    pub(crate) fn begin_pending_steer_withdrawal(
        &mut self,
        client_user_message_id: &str,
        request_id: &str,
    ) -> Option<String> {
        let turn_id = begin_pending_steer_withdrawal(
            &mut self.input_queue.pending_steers,
            client_user_message_id,
            request_id,
        );
        if turn_id.is_some() {
            self.refresh_pending_input_preview();
        }
        turn_id
    }

    pub(crate) fn reconcile_pending_steer_withdrawal(
        &mut self,
        client_user_message_id: &str,
        accepted_turn_id: &str,
        request_id: &str,
        outcome: PendingSteerWithdrawalOutcome,
    ) -> PendingSteerWithdrawalEffect {
        let effect = reconcile_pending_steer_withdrawal(
            &mut self.input_queue.pending_steers,
            client_user_message_id,
            accepted_turn_id,
            request_id,
            outcome,
        );
        if effect != PendingSteerWithdrawalEffect::Noop {
            self.refresh_pending_input_preview();
        }
        effect
    }
}

#[cfg(test)]
#[path = "pending_steer_withdrawal_tests.rs"]
mod tests;
