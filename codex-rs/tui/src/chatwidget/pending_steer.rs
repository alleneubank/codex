//! Stable identity and retained display state for messages submitted into an active turn.

use std::collections::VecDeque;

use super::ThreadInputState;
use super::UserMessage;
use super::UserMessageHistoryRecord;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PendingSteer {
    pub(crate) client_user_message_id: String,
    pub(super) user_message: UserMessage,
    pub(super) history_record: UserMessageHistoryRecord,
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

impl ThreadInputState {
    pub(crate) fn reconcile_committed_pending_steer(&mut self, client_user_message_id: &str) {
        if let Some(committed) =
            take_pending_steer(&mut self.pending_steers, client_user_message_id)
        {
            self.committed_steers_for_replay.push_back(committed);
        }
    }
}
