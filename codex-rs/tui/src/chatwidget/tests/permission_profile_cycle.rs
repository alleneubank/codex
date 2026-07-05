use super::*;
use crate::legacy_core::config::PermissionProfileCatalogEntry;
use crate::legacy_core::config::PermissionProfileSnapshot;
use codex_protocol::models::ActivePermissionProfile;
use codex_protocol::models::BUILT_IN_PERMISSION_PROFILE_DANGER_FULL_ACCESS;
use codex_protocol::models::BUILT_IN_PERMISSION_PROFILE_READ_ONLY;

fn next_profile_selection(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
) -> PermissionProfileSelection {
    std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|event| match event {
            AppEvent::SelectPermissionProfile(selection) => Some(selection),
            _ => None,
        })
        .expect("expected permission profile selection")
}

fn set_active_profile(
    chat: &mut ChatWidget,
    permission_profile: PermissionProfile,
    profile_id: &str,
) {
    chat.config
        .permissions
        .set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
            permission_profile,
            ActivePermissionProfile::new(profile_id),
        ))
        .expect("set active profile");
}

#[tokio::test]
async fn profile_cycle_wraps_from_last_named_profile_to_ask_for_approval() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.explicit_permission_profile_mode = true;
    chat.config.custom_permission_profiles = vec![PermissionProfileCatalogEntry {
        id: "locked-down".to_string(),
        description: None,
        allowed: true,
    }];
    set_active_profile(
        &mut chat,
        PermissionProfile::workspace_write(),
        "locked-down",
    );
    chat.set_approvals_reviewer(ApprovalsReviewer::User);

    chat.cycle_permission_mode_from_keybinding();

    let selection = next_profile_selection(&mut rx);
    assert_eq!(selection.profile_id, BUILT_IN_PERMISSION_PROFILE_WORKSPACE);
    assert_eq!(selection.approval_policy, Some(AskForApproval::OnRequest));
    assert_eq!(selection.approvals_reviewer, Some(ApprovalsReviewer::User));
    assert_eq!(selection.display_label, ASK_FOR_APPROVAL_LABEL);
}

#[tokio::test]
async fn profile_cycle_skips_disallowed_builtin_and_custom_options() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.explicit_permission_profile_mode = true;
    chat.set_feature_enabled(Feature::GuardianApproval, /*enabled*/ true);
    chat.config.config_layer_stack =
        super::permissions::requirements_stack(codex_config::ConfigRequirementsToml {
            allowed_approvals_reviewers: Some(vec![ApprovalsReviewer::User]),
            allowed_sandbox_modes: Some(vec![
                codex_config::SandboxModeRequirement::ReadOnly,
                codex_config::SandboxModeRequirement::WorkspaceWrite,
            ]),
            ..Default::default()
        });
    chat.config.custom_permission_profiles = vec![
        PermissionProfileCatalogEntry {
            id: "disabled-profile".to_string(),
            description: None,
            allowed: false,
        },
        PermissionProfileCatalogEntry {
            id: "locked-down".to_string(),
            description: None,
            allowed: true,
        },
    ];
    set_active_profile(
        &mut chat,
        PermissionProfile::workspace_write(),
        BUILT_IN_PERMISSION_PROFILE_WORKSPACE,
    );
    chat.set_approvals_reviewer(ApprovalsReviewer::User);

    chat.cycle_permission_mode_from_keybinding();

    let read_only = next_profile_selection(&mut rx);
    assert_eq!(read_only.profile_id, BUILT_IN_PERMISSION_PROFILE_READ_ONLY);
    assert_eq!(read_only.approval_policy, Some(AskForApproval::OnRequest));
    assert_eq!(read_only.approvals_reviewer, Some(ApprovalsReviewer::User));
    assert_eq!(read_only.display_label, "Read Only");

    set_active_profile(
        &mut chat,
        PermissionProfile::read_only(),
        BUILT_IN_PERMISSION_PROFILE_READ_ONLY,
    );
    chat.cycle_permission_mode_from_keybinding();

    let custom = next_profile_selection(&mut rx);
    assert_eq!(custom.profile_id, "locked-down");
    assert_eq!(custom.approval_policy, None);
    assert_eq!(custom.approvals_reviewer, None);
    assert_eq!(custom.display_label, "locked-down");
}

#[tokio::test]
async fn profile_cycle_routes_full_access_through_confirmation() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.explicit_permission_profile_mode = true;
    chat.set_feature_enabled(Feature::GuardianApproval, /*enabled*/ false);
    chat.config.notices.hide_full_access_warning = None;
    set_active_profile(
        &mut chat,
        PermissionProfile::workspace_write(),
        BUILT_IN_PERMISSION_PROFILE_WORKSPACE,
    );
    chat.set_approvals_reviewer(ApprovalsReviewer::User);

    chat.cycle_permission_mode_from_keybinding();

    let (preset, return_to_permissions, profile_selection) =
        std::iter::from_fn(|| rx.try_recv().ok())
            .find_map(|event| match event {
                AppEvent::OpenFullAccessConfirmation {
                    preset,
                    return_to_permissions,
                    profile_selection,
                } => Some((preset, return_to_permissions, profile_selection)),
                _ => None,
            })
            .expect("expected full-access confirmation");
    assert_eq!(preset.id, "full-access");
    assert!(return_to_permissions);
    let selection = profile_selection.expect("expected profile selection");
    assert_eq!(
        selection.profile_id,
        BUILT_IN_PERMISSION_PROFILE_DANGER_FULL_ACCESS
    );
    assert_eq!(selection.approval_policy, Some(AskForApproval::Never));
    assert_eq!(selection.approvals_reviewer, Some(ApprovalsReviewer::User));
    assert_eq!(selection.display_label, "Full Access");
}

#[tokio::test]
async fn profile_picker_disables_reviewer_disallowed_auto_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.explicit_permission_profile_mode = true;
    chat.set_feature_enabled(Feature::GuardianApproval, /*enabled*/ true);
    chat.config.config_layer_stack =
        super::permissions::requirements_stack(codex_config::ConfigRequirementsToml {
            allowed_approvals_reviewers: Some(vec![ApprovalsReviewer::User]),
            ..Default::default()
        });

    chat.open_permissions_popup();

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        popup.contains("Auto (disabled)"),
        "expected Auto to be disabled by reviewer requirements: {popup}"
    );
    assert_chatwidget_snapshot!(
        "profile_permissions_selection_popup_with_disallowed_auto",
        popup
    );
}
