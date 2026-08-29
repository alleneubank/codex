//! Source-thread orchestration for one pending-steer withdrawal request.

use super::*;
use crate::chatwidget::PendingSteerWithdrawalEffect;
use crate::chatwidget::PendingSteerWithdrawalOutcome;
use codex_app_server_protocol::WarningNotification;

impl App {
    pub(super) async fn reconcile_pending_steer_withdrawal_response(
        &mut self,
        source_thread_id: ThreadId,
        accepted_turn_id: &str,
        client_user_message_id: &str,
        request_id: &str,
        result: PendingSteerWithdrawalRequestResult,
    ) -> Result<PendingSteerWithdrawalEffect> {
        let (outcome, diagnostic) = match result {
            PendingSteerWithdrawalRequestResult::Withdrawn { turn_id }
                if turn_id == accepted_turn_id =>
            {
                (PendingSteerWithdrawalOutcome::Withdrawn, None)
            }
            PendingSteerWithdrawalRequestResult::Withdrawn { .. } => {
                return Ok(PendingSteerWithdrawalEffect::Noop);
            }
            PendingSteerWithdrawalRequestResult::Rejected(error) => (
                PendingSteerWithdrawalOutcome::Rejected,
                Some(format!(
                    "Could not edit the pending message. It remains pending; try again: {}",
                    error.message
                )),
            ),
            PendingSteerWithdrawalRequestResult::Uncertain(error) => (
                PendingSteerWithdrawalOutcome::Uncertain,
                Some(format!(
                    "Could not confirm whether pending-message editing succeeded. Editing stays disabled until delivery or interruption is confirmed: {error}"
                )),
            ),
        };
        let source_is_displayed = self.active_thread_id == Some(source_thread_id)
            && self.chat_widget.thread_id() == Some(source_thread_id);
        let effect = if source_is_displayed {
            self.chat_widget.reconcile_pending_steer_withdrawal(
                client_user_message_id,
                accepted_turn_id,
                request_id,
                outcome,
            )
        } else {
            self.reconcile_stored_pending_steer_withdrawal(
                source_thread_id,
                client_user_message_id,
                accepted_turn_id,
                request_id,
                outcome,
            )
            .await
        };
        if matches!(
            effect,
            PendingSteerWithdrawalEffect::Rejected | PendingSteerWithdrawalEffect::BecameUncertain
        ) && let Some(message) = diagnostic
        {
            self.enqueue_thread_notification(
                source_thread_id,
                ServerNotification::Warning(WarningNotification {
                    thread_id: Some(source_thread_id.to_string()),
                    message,
                }),
            )
            .await?;
        }
        Ok(effect)
    }

    async fn reconcile_stored_pending_steer_withdrawal(
        &mut self,
        source_thread_id: ThreadId,
        client_user_message_id: &str,
        accepted_turn_id: &str,
        request_id: &str,
        outcome: PendingSteerWithdrawalOutcome,
    ) -> PendingSteerWithdrawalEffect {
        let mut effect = PendingSteerWithdrawalEffect::Noop;
        if let Some(channel) = self.thread_event_channels.get(&source_thread_id) {
            let mut store = channel.store.lock().await;
            if let Some(input_state) = store.input_state.as_mut() {
                effect = input_state.reconcile_pending_steer_withdrawal(
                    client_user_message_id,
                    accepted_turn_id,
                    request_id,
                    outcome,
                );
                self.agents_overview
                    .input_states
                    .insert(source_thread_id, input_state.clone());
            }
        } else if let Some(input_state) =
            self.agents_overview.input_states.get_mut(&source_thread_id)
        {
            effect = input_state.reconcile_pending_steer_withdrawal(
                client_user_message_id,
                accepted_turn_id,
                request_id,
                outcome,
            );
        }
        effect
    }
}
