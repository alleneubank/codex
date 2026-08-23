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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionReasoningEffortState {
    pub(crate) default_effort: Option<ReasoningEffortConfig>,
    pub(crate) plan_effort_override: Option<Option<ReasoningEffortConfig>>,
}

impl ChatWidget {
    pub(crate) fn session_reasoning_effort_state(&self) -> SessionReasoningEffortState {
        SessionReasoningEffortState {
            default_effort: self.current_collaboration_mode.reasoning_effort(),
            plan_effort_override: self.session_plan_mode_reasoning_effort.clone(),
        }
    }

    pub(crate) fn restore_session_reasoning_effort_state(
        &mut self,
        state: SessionReasoningEffortState,
    ) {
        self.current_collaboration_mode = self.current_collaboration_mode.with_updates(
            /*model*/ None,
            Some(state.default_effort.clone()),
            /*developer_instructions*/ None,
        );
        self.session_plan_mode_reasoning_effort = state.plan_effort_override;
        if let Some(mut mask) = self.active_collaboration_mask.take() {
            if mask.mode == Some(ModeKind::Plan) {
                self.apply_plan_mode_reasoning_effort_override(&mut mask);
            } else {
                mask.reasoning_effort = Some(state.default_effort);
            }
            self.active_collaboration_mask = Some(mask);
        }
        self.refresh_model_dependent_surfaces();
    }

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
        effort: Option<ReasoningEffortConfig>,
    ) -> Vec<SelectionAction> {
        let update_plan_mode =
            self.collaboration_modes_enabled() && self.active_mode_kind() == ModeKind::Plan;
        let warning = effort
            .as_ref()
            .and_then(|effort| self.ultra_reasoning_concurrency_warning(effort));
        vec![Box::new(move |tx| {
            if update_plan_mode {
                tx.send(AppEvent::UpdateSessionPlanModeReasoningEffort(
                    effort.clone(),
                ));
            } else {
                tx.send(AppEvent::UpdateSessionReasoningEffort(effort.clone()));
            }
            if let Some(warning) = warning.clone() {
                tx.send(AppEvent::InsertHistoryCell(Box::new(
                    history_cell::new_warning_event(warning),
                )));
            }
        })]
    }
}
