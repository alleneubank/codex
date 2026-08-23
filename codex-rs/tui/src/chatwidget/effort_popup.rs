//! Session-only reasoning-effort selection for `ChatWidget`.
//!
//! The shared model picker can update persisted defaults. `/effort` must not,
//! so this module owns the narrower entry point and the actions that update only
//! the active default-mode or Plan-mode session state.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ReasoningSelectionScope {
    PersistedDefault,
    CurrentSession,
}

impl ChatWidget {
    /// Open a picker for changing only the active session's reasoning effort.
    pub(crate) fn open_effort_popup(&mut self) {
        if !self.is_session_configured() {
            self.add_info_message(
                "Effort selection is disabled until startup completes.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let current_model = self.current_model().to_string();
        let Some(preset) = self.current_model_preset() else {
            self.add_info_message(
                format!("Reasoning effort selection is unavailable for {current_model}."),
                /*hint*/ None,
            );
            return;
        };
        self.open_reasoning_popup_for_scope(preset, ReasoningSelectionScope::CurrentSession);
    }

    pub(super) fn session_reasoning_selection_actions(
        &self,
        model: String,
        effort: Option<ReasoningEffortConfig>,
    ) -> Vec<SelectionAction> {
        if let Some(selection) = self.session_model_selection_action(model, effort.clone()) {
            return vec![selection.action];
        }
        // Reserve selections already use the upstream thread-scoped action.
        let thread_id = self.thread_id();
        vec![Box::new(move |tx| {
            if let Some(thread_id) = thread_id {
                tx.send(AppEvent::UpdateLunaReserveReasoning {
                    thread_id,
                    effort: effort.clone(),
                });
            }
        })]
    }
}
