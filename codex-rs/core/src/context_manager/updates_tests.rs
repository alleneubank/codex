use super::*;
use crate::config::Constrained;
use codex_execpolicy::Policy;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_protocol::protocol::AskForApproval;
use std::sync::Arc;

#[tokio::test]
async fn permissions_update_emits_when_only_approvals_reviewer_changes() {
    let (_session, mut next) = crate::session::tests::make_session_and_context().await;
    let previous = next.to_turn_context_item();
    let previous_turn_settings = PreviousTurnSettings {
        model: next.model_info.slug.clone(),
        comp_hash: next.model_info.comp_hash.clone(),
        realtime_active: Some(next.realtime_active),
        permission_profile: codex_protocol::models::PermissionProfile::read_only(),
        approval_policy: codex_protocol::protocol::AskForApproval::OnRequest,
        approvals_reviewer: Some(ApprovalsReviewer::User),
    };
    let mut next_config = (*next.config).clone();
    next_config.approvals_reviewer = ApprovalsReviewer::AutoReview;
    next.config = Arc::new(next_config);

    let update = build_permissions_update_item(
        Some(&previous),
        Some(&previous_turn_settings),
        &next,
        &Policy::empty(),
    )
    .expect("reviewer-only changes should refresh permissions instructions");

    assert!(
        update.contains("`approvals_reviewer` is `auto_review`"),
        "{update}"
    );
}

#[tokio::test]
async fn permissions_update_uses_turn_context_reviewer_without_previous_turn_settings() {
    let (_session, mut next) = crate::session::tests::make_session_and_context().await;
    let mut next_config = (*next.config).clone();
    next_config.approvals_reviewer = ApprovalsReviewer::AutoReview;
    next.config = Arc::new(next_config);
    let previous = next.to_turn_context_item();

    let update = build_permissions_update_item(Some(&previous), None, &next, &Policy::empty());

    assert_eq!(update, None);
}

#[tokio::test]
async fn permissions_update_emits_when_previous_reviewer_is_unknown() {
    let (_session, next) = crate::session::tests::make_session_and_context().await;
    let mut previous = next.to_turn_context_item();
    previous.approvals_reviewer = None;

    let update = build_permissions_update_item(Some(&previous), None, &next, &Policy::empty())
        .expect("legacy missing reviewer should refresh permissions instructions");

    assert!(update.contains("<permissions instructions>"), "{update}");
}

#[tokio::test]
async fn permissions_update_omits_reviewer_only_change_when_rendered_text_is_same() {
    let (_session, mut next) = crate::session::tests::make_session_and_context().await;
    let mut next_config = (*next.config).clone();
    next_config.permissions.approval_policy = Constrained::allow_any(AskForApproval::Never);
    next_config.approvals_reviewer = ApprovalsReviewer::AutoReview;
    next.config = Arc::new(next_config);
    next.approval_policy = Constrained::allow_any(AskForApproval::Never);
    let mut previous = next.to_turn_context_item();
    previous.approvals_reviewer = Some(ApprovalsReviewer::User);

    let update = build_permissions_update_item(Some(&previous), None, &next, &Policy::empty());

    assert_eq!(update, None);
}
