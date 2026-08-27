use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use codex_app_server_protocol::ClientResponsePayload;
use codex_app_server_protocol::ThreadUserAttentionCompleteParams;
use codex_app_server_protocol::ThreadUserAttentionCompleteResponse;
use codex_app_server_protocol::ThreadUserAttentionStartParams;
use codex_app_server_protocol::ThreadUserAttentionStartResponse;
use codex_app_server_protocol::UserAttentionKind;
use codex_core::UserAttentionLifecycle;
use codex_protocol::ThreadId;

use crate::error_code::internal_error;
use crate::error_code::invalid_request;
use crate::outgoing_message::ConnectionId;

use super::JSONRPCErrorError;
use super::ThreadRequestProcessor;

const MAX_ACTIVE_USER_ATTENTION_PER_CONNECTION: usize = 32;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct UserAttentionKey {
    connection_id: ConnectionId,
    thread_id: ThreadId,
    attention_id: String,
}

#[derive(Clone, Default)]
pub(super) struct UserAttentionManager {
    lifecycles: Arc<Mutex<HashMap<UserAttentionKey, Option<UserAttentionLifecycle>>>>,
}

struct UserAttentionReservation {
    key: UserAttentionKey,
    lifecycles: Arc<Mutex<HashMap<UserAttentionKey, Option<UserAttentionLifecycle>>>>,
    committed: bool,
}

impl UserAttentionReservation {
    fn commit(mut self, lifecycle: UserAttentionLifecycle) {
        let mut lifecycles = self
            .lifecycles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(slot) = lifecycles.get_mut(&self.key) {
            *slot = Some(lifecycle);
        }
        self.committed = true;
    }
}

impl Drop for UserAttentionReservation {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        self.lifecycles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.key);
    }
}

impl UserAttentionManager {
    fn reserve(
        &self,
        key: UserAttentionKey,
    ) -> Result<UserAttentionReservation, JSONRPCErrorError> {
        let mut lifecycles = self
            .lifecycles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if lifecycles.contains_key(&key) {
            return Err(invalid_request("attentionId is already active"));
        }
        let active_for_connection = lifecycles
            .keys()
            .filter(|active| active.connection_id == key.connection_id)
            .count();
        if active_for_connection >= MAX_ACTIVE_USER_ATTENTION_PER_CONNECTION {
            return Err(invalid_request("too many active user-attention lifecycles"));
        }
        lifecycles.insert(key.clone(), None);
        drop(lifecycles);
        Ok(UserAttentionReservation {
            key,
            lifecycles: Arc::clone(&self.lifecycles),
            committed: false,
        })
    }

    async fn complete_matching(&self, predicate: impl Fn(&UserAttentionKey) -> bool) {
        let lifecycles = {
            let mut active = self
                .lifecycles
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let keys = active
                .keys()
                .filter(|key| predicate(key))
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| active.remove(&key).flatten())
                .collect::<Vec<_>>()
        };
        for lifecycle in lifecycles {
            lifecycle.complete().await;
        }
    }

    pub(super) async fn complete_connection(&self, connection_id: ConnectionId) {
        self.complete_matching(|key| key.connection_id == connection_id)
            .await;
    }

    pub(super) async fn complete_thread(&self, thread_id: ThreadId) {
        self.complete_matching(|key| key.thread_id == thread_id)
            .await;
    }
}

impl ThreadRequestProcessor {
    pub(crate) async fn thread_user_attention_start(
        &self,
        connection_id: ConnectionId,
        params: ThreadUserAttentionStartParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let thread_id = ThreadId::from_string(&params.thread_id)
            .map_err(|error| invalid_request(format!("invalid thread id: {error}")))?;
        let key = UserAttentionKey {
            connection_id,
            thread_id,
            attention_id: params.attention_id,
        };
        let reservation = self.user_attention.reserve(key)?;
        let thread = self
            .thread_manager
            .get_thread(thread_id)
            .await
            .map_err(|_| invalid_request(format!("thread not found: {thread_id}")))?;
        let lifecycle = match params.kind {
            UserAttentionKind::PlanImplementation => thread
                .start_plan_implementation_attention(&params.turn_id)
                .await
                .map_err(|error| invalid_request(error.to_string()))?,
        };
        reservation.commit(lifecycle);
        Ok(Some(ThreadUserAttentionStartResponse::default().into()))
    }

    pub(crate) async fn thread_user_attention_complete(
        &self,
        connection_id: ConnectionId,
        params: ThreadUserAttentionCompleteParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let thread_id = ThreadId::from_string(&params.thread_id)
            .map_err(|error| invalid_request(format!("invalid thread id: {error}")))?;
        let key = UserAttentionKey {
            connection_id,
            thread_id,
            attention_id: params.attention_id,
        };
        let lifecycle = self
            .user_attention
            .lifecycles
            .lock()
            .map_err(|_| internal_error("user-attention registry lock is poisoned"))?
            .remove(&key)
            .flatten();
        if let Some(lifecycle) = lifecycle {
            lifecycle.complete().await;
        }
        Ok(Some(ThreadUserAttentionCompleteResponse::default().into()))
    }
}
