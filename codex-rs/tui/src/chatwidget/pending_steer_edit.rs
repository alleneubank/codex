//! Plain-Up interaction and confirmed pending-steer restore behavior.

use super::*;
use uuid::Uuid;

impl ChatWidget {
    pub(super) fn handle_pending_steer_edit_key(&mut self, key_event: KeyEvent) -> bool {
        if !matches!(
            key_event,
            KeyEvent {
                code: KeyCode::Up,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                ..
            }
        ) || !self.bottom_pane.composer_is_empty()
            || !self.bottom_pane.no_modal_or_popup_active()
        {
            return false;
        }
        if self.input_queue.pending_steers.iter().any(|pending| {
            matches!(
                pending.lifecycle,
                PendingSteerLifecycle::WithdrawalInFlight { .. }
                    | PendingSteerLifecycle::WithdrawalUncertain { .. }
            )
        }) {
            return true;
        }
        let Some(client_user_message_id) = self
            .input_queue
            .pending_steers
            .iter()
            .rev()
            .find(|pending| matches!(pending.lifecycle, PendingSteerLifecycle::Accepted { .. }))
            .map(|pending| pending.client_user_message_id.clone())
        else {
            return false;
        };
        let Some(source_thread_id) = self.thread_id else {
            return false;
        };
        let request_id = Uuid::new_v4().to_string();
        let Some(accepted_turn_id) =
            self.begin_pending_steer_withdrawal(&client_user_message_id, &request_id)
        else {
            return false;
        };
        if !self.submit_op(AppCommand::WithdrawPendingSteer {
            source_thread_id,
            accepted_turn_id: accepted_turn_id.clone(),
            client_user_message_id: client_user_message_id.clone(),
            request_id: request_id.clone(),
        }) {
            self.reconcile_pending_steer_withdrawal(
                &client_user_message_id,
                &accepted_turn_id,
                &request_id,
                PendingSteerWithdrawalOutcome::Rejected,
            );
        }
        true
    }

    pub(crate) fn restore_withdrawn_pending_steer(&mut self, pending: PendingSteer) {
        self.restore_composer_state(composer_state_for_withdrawn_pending_steer(pending));
        self.refresh_pending_input_preview();
        self.request_redraw();
    }
}

impl ThreadInputState {
    pub(crate) fn restore_withdrawn_pending_steer(&mut self, pending: PendingSteer) {
        self.composer = Some(composer_state_for_withdrawn_pending_steer(pending));
    }
}

fn composer_state_for_withdrawn_pending_steer(pending: PendingSteer) -> ThreadComposerState {
    let PendingSteer {
        user_message,
        history_record,
        ..
    } = pending;
    let user_message = user_message_for_restore(user_message, &history_record);
    let cursor = user_message.text.len();
    ThreadComposerState {
        text: user_message.text,
        local_images: user_message.local_images,
        remote_image_urls: user_message.remote_image_urls,
        text_elements: user_message.text_elements,
        mention_bindings: user_message.mention_bindings,
        pending_pastes: Vec::new(),
        cursor,
    }
}
