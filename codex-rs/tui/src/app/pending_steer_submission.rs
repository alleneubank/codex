//! Source-thread reconciliation for pending-steer submission outcomes.

use super::*;
use crate::chatwidget::PendingSteerSubmissionEffect;
use crate::chatwidget::PendingSteerSubmissionOutcome;
use codex_app_server_protocol::WarningNotification;

pub(super) enum PendingSteerSubmissionDiagnostic {
    Warning(String),
    Error(String),
}

impl App {
    pub(super) async fn reconcile_pending_steer_submission(
        &mut self,
        source_thread_id: ThreadId,
        client_user_message_id: &str,
        outcome: PendingSteerSubmissionOutcome,
        diagnostic: Option<PendingSteerSubmissionDiagnostic>,
    ) -> Result<PendingSteerSubmissionEffect> {
        let source_is_displayed = self.active_thread_id == Some(source_thread_id)
            && self.chat_widget.thread_id() == Some(source_thread_id);
        let effect = if source_is_displayed {
            self.chat_widget
                .reconcile_pending_steer_submission(client_user_message_id, outcome)
        } else {
            self.reconcile_stored_pending_steer_submission(
                source_thread_id,
                client_user_message_id,
                outcome,
            )
            .await
        };

        match (effect, diagnostic) {
            (
                PendingSteerSubmissionEffect::BecameUncertain,
                Some(PendingSteerSubmissionDiagnostic::Warning(message)),
            ) => {
                self.enqueue_thread_notification(
                    source_thread_id,
                    ServerNotification::Warning(WarningNotification {
                        thread_id: Some(source_thread_id.to_string()),
                        message,
                    }),
                )
                .await?;
            }
            (
                PendingSteerSubmissionEffect::Rejected,
                Some(PendingSteerSubmissionDiagnostic::Error(message)),
            ) => {
                self.enqueue_thread_local_error(source_thread_id, message)
                    .await
            }
            (
                PendingSteerSubmissionEffect::Noop
                | PendingSteerSubmissionEffect::Updated
                | PendingSteerSubmissionEffect::Rejected
                | PendingSteerSubmissionEffect::BecameUncertain,
                _,
            ) => {}
        }
        Ok(effect)
    }

    pub(super) async fn enqueue_thread_local_error(
        &mut self,
        thread_id: ThreadId,
        message: String,
    ) {
        let (sender, store) = {
            let channel = self.ensure_thread_channel(thread_id);
            (channel.sender.clone(), Arc::clone(&channel.store))
        };
        let event = ThreadBufferedEvent::LocalError(message);
        let should_send = {
            let mut store = store.lock().await;
            store.push_buffered_event(event.clone());
            store.active
        };
        if should_send {
            match sender.try_send(event) {
                Ok(()) => {}
                Err(TrySendError::Full(event)) => {
                    tokio::spawn(async move {
                        if let Err(error) = sender.send(event).await {
                            tracing::warn!(%thread_id, %error, "thread-local error channel closed");
                        }
                    });
                }
                Err(TrySendError::Closed(_)) => {
                    tracing::warn!(%thread_id, "thread-local error channel closed");
                }
            }
        }
    }

    async fn reconcile_stored_pending_steer_submission(
        &mut self,
        source_thread_id: ThreadId,
        client_user_message_id: &str,
        outcome: PendingSteerSubmissionOutcome,
    ) -> PendingSteerSubmissionEffect {
        let mut canonical_input_state = None;
        let mut effect = PendingSteerSubmissionEffect::Noop;
        if let Some(channel) = self.thread_event_channels.get(&source_thread_id) {
            let mut store = channel.store.lock().await;
            if let Some(input_state) = store.input_state.as_mut() {
                effect = input_state
                    .reconcile_pending_steer_submission(client_user_message_id, outcome.clone());
                canonical_input_state = Some(input_state.clone());
            }
        }

        if let Some(input_state) = canonical_input_state {
            if self
                .agents_overview
                .input_states
                .contains_key(&source_thread_id)
            {
                self.agents_overview
                    .input_states
                    .insert(source_thread_id, input_state);
            }
        } else if let Some(input_state) =
            self.agents_overview.input_states.get_mut(&source_thread_id)
        {
            effect =
                input_state.reconcile_pending_steer_submission(client_user_message_id, outcome);
        }
        effect
    }
}
