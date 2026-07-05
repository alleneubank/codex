//! Permission option construction and selection surfaces.
//!
//! Explicit permission-profile mode uses one ordered option model for both
//! the `/permissions` picker and keybinding-driven cycling. Keeping disabled
//! state and activation behavior on that model prevents the two surfaces from
//! drifting apart.

use super::*;

enum PermissionOptionKind {
    BuiltIn {
        preset: ApprovalPreset,
        approvals_reviewer: ApprovalsReviewer,
    },
    NamedProfile {
        profile_id: String,
    },
}

struct PermissionOption {
    label: String,
    description: String,
    disabled_reason: Option<String>,
    kind: PermissionOptionKind,
}

impl PermissionOption {
    fn selection(&self) -> PermissionProfileSelection {
        match &self.kind {
            PermissionOptionKind::BuiltIn {
                preset,
                approvals_reviewer,
            } => PermissionProfileSelection {
                profile_id: preset.active_permission_profile.id.clone(),
                approval_policy: Some(AskForApproval::from(preset.approval)),
                approvals_reviewer: Some(*approvals_reviewer),
                display_label: self.label.clone(),
            },
            PermissionOptionKind::NamedProfile { profile_id } => PermissionProfileSelection {
                profile_id: profile_id.clone(),
                approval_policy: None,
                approvals_reviewer: None,
                display_label: self.label.clone(),
            },
        }
    }

    fn is_current(&self, chat: &ChatWidget) -> bool {
        let active_profile_id = chat
            .config
            .permissions
            .active_permission_profile()
            .map(|profile| profile.id);
        match &self.kind {
            PermissionOptionKind::BuiltIn {
                preset,
                approvals_reviewer,
            } => {
                active_profile_id.as_deref() == Some(preset.active_permission_profile.id.as_str())
                    && AskForApproval::from(chat.config.permissions.approval_policy.value())
                        == AskForApproval::from(preset.approval)
                    && chat.config.approvals_reviewer == *approvals_reviewer
            }
            PermissionOptionKind::NamedProfile { profile_id } => {
                active_profile_id.as_deref() == Some(profile_id.as_str())
            }
        }
    }

    fn actions(&self, chat: &ChatWidget) -> Vec<SelectionAction> {
        match &self.kind {
            PermissionOptionKind::BuiltIn {
                preset,
                approvals_reviewer,
            } => chat.permission_mode_actions(
                preset,
                self.label.clone(),
                *approvals_reviewer,
                Some(self.selection()),
                /*return_to_permissions*/ true,
            ),
            PermissionOptionKind::NamedProfile { .. } => {
                ChatWidget::permission_profile_selection_actions(self.selection())
            }
        }
    }

    fn selection_item(&self, chat: &ChatWidget) -> SelectionItem {
        SelectionItem {
            name: self.label.clone(),
            description: Some(self.description.clone()),
            is_current: self.is_current(chat),
            actions: self.actions(chat),
            dismiss_on_select: true,
            disabled_reason: self.disabled_reason.clone(),
            ..Default::default()
        }
    }

    fn is_enabled(&self) -> bool {
        self.disabled_reason.is_none()
    }
}

impl ChatWidget {
    pub(super) fn open_permission_profiles_popup(&mut self) {
        let options = match self.permission_profile_options() {
            Ok(options) => options,
            Err(err) => {
                self.add_error_message(err);
                return;
            }
        };
        let items = options
            .iter()
            .map(|option| option.selection_item(self))
            .collect();

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Update Model Permissions".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            header: Box::new(()),
            ..Default::default()
        });
    }

    fn permission_profile_options(&self) -> Result<Vec<PermissionOption>, String> {
        let presets = builtin_approval_presets();
        let find_preset = |id| {
            presets
                .iter()
                .find(|preset| preset.id == id)
                .cloned()
                .ok_or_else(|| format!("Internal error: missing the '{id}' approval preset."))
        };
        let default = find_preset("auto")?;
        let full_access = find_preset("full-access")?;
        let read_only = find_preset("read-only")?;

        let mut options = vec![
            self.builtin_permission_option(
                default.clone(),
                ASK_FOR_APPROVAL_LABEL,
                default
                    .description
                    .replace(" (Identical to Agent mode)", ""),
                ApprovalsReviewer::User,
            ),
        ];
        if self.config.features.enabled(Feature::GuardianApproval) {
            options.push(self.builtin_permission_option(
                default,
                AUTO_LABEL,
                AUTO_REVIEW_DESCRIPTION.to_string(),
                ApprovalsReviewer::AutoReview,
            ));
        }
        options.push(self.builtin_permission_option(
            full_access.clone(),
            full_access.label,
            full_access.description.to_string(),
            ApprovalsReviewer::User,
        ));
        options.push(self.builtin_permission_option(
            read_only.clone(),
            read_only.label,
            read_only.description.to_string(),
            ApprovalsReviewer::User,
        ));
        options.extend(
            self.config
                .custom_permission_profiles
                .iter()
                .map(|profile| PermissionOption {
                    label: profile.id.clone(),
                    description: profile
                        .description
                        .clone()
                        .unwrap_or_else(|| "Configured permission profile.".to_string()),
                    disabled_reason: (!profile.allowed)
                        .then(|| "Disabled by requirements.".to_string()),
                    kind: PermissionOptionKind::NamedProfile {
                        profile_id: profile.id.clone(),
                    },
                }),
        );
        Ok(options)
    }

    fn builtin_permission_option(
        &self,
        preset: ApprovalPreset,
        label: &str,
        description: String,
        approvals_reviewer: ApprovalsReviewer,
    ) -> PermissionOption {
        let approval_policy = AskForApproval::from(preset.approval);
        let disabled_reason = self
            .config
            .permissions
            .approval_policy
            .can_set(&approval_policy.to_core())
            .err()
            .map(|err| err.to_string())
            .or_else(|| {
                self.config
                    .config_layer_stack
                    .requirements()
                    .approvals_reviewer
                    .can_set(&approvals_reviewer)
                    .err()
                    .map(|err| err.to_string())
            })
            .or_else(|| {
                self.config
                    .permissions
                    .can_set_permission_profile(&preset.permission_profile)
                    .err()
                    .map(|err| err.to_string())
            })
            .or_else(|| {
                (!self.config.is_permission_profile_allowed(
                    preset.active_permission_profile.id.as_str(),
                    &preset.permission_profile,
                ))
                .then(|| "Disabled by requirements.".to_string())
            });
        PermissionOption {
            label: label.to_string(),
            description,
            disabled_reason,
            kind: PermissionOptionKind::BuiltIn {
                preset,
                approvals_reviewer,
            },
        }
    }

    pub(crate) fn cycle_permission_mode_from_keybinding(&mut self) {
        if self.config.explicit_permission_profile_mode {
            self.cycle_permission_profile_from_keybinding();
        } else {
            self.cycle_legacy_permission_mode_from_keybinding();
        }
    }

    fn cycle_permission_profile_from_keybinding(&mut self) {
        let options = match self.permission_profile_options() {
            Ok(options) => options,
            Err(err) => {
                self.add_error_message(err);
                return;
            }
        };
        let current_index = options.iter().position(|option| option.is_current(self));
        let start_index = current_index.map_or(0, |index| index + 1);
        let next_option = (0..options.len())
            .map(|offset| &options[(start_index + offset) % options.len()])
            .find(|option| option.is_enabled());
        let Some(next_option) = next_option else {
            self.add_error_message("No permission modes are available.".to_string());
            return;
        };

        for action in next_option.actions(self) {
            action(&self.app_event_tx);
        }
    }
}
