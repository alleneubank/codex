use super::*;
use codex_arg0::Arg0DispatchPaths;
use codex_protocol::models::BUILT_IN_PERMISSION_PROFILE_DANGER_FULL_ACCESS;
use pretty_assertions::assert_eq;

fn read_only_selection() -> PermissionProfileSelection {
    PermissionProfileSelection {
        profile_id: ":read-only".to_string(),
        approval_policy: Some(AskForApproval::OnRequest),
        approvals_reviewer: Some(ApprovalsReviewer::User),
        display_label: "Read Only".to_string(),
    }
}

fn full_access_selection() -> PermissionProfileSelection {
    PermissionProfileSelection {
        profile_id: BUILT_IN_PERMISSION_PROFILE_DANGER_FULL_ACCESS.to_string(),
        approval_policy: Some(AskForApproval::Never),
        approvals_reviewer: Some(ApprovalsReviewer::User),
        display_label: "Full Access".to_string(),
    }
}

struct ConfirmedPermissionShortcutCase {
    selection: PermissionProfileSelection,
    expected_profile: PermissionProfile,
    expected_sandbox_policy: codex_app_server_protocol::SandboxPolicy,
}

async fn assert_permission_shortcut_confirms_without_persisting(
    case: ConfirmedPermissionShortcutCase,
) -> Result<String> {
    let expected_profile_id = case.selection.profile_id.clone();
    let expected_approval = case
        .selection
        .approval_policy
        .expect("built-in shortcut approval policy");
    let expected_reviewer = case
        .selection
        .approvals_reviewer
        .expect("built-in shortcut approvals reviewer");
    let expected_app_reviewer: codex_app_server_protocol::ApprovalsReviewer =
        expected_reviewer.into();
    let (mut app, mut events, _op_rx) = make_test_app_with_channels().await;
    let codex_home = tempdir()?;
    app.config.codex_home = codex_home.path().to_path_buf().abs();
    app.config
        .permissions
        .set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
            PermissionProfile::workspace_write(),
            ActivePermissionProfile::new(":workspace"),
        ))?;
    let config_path = codex_home.path().join("config.toml");
    let contents = "approvals_reviewer = \"auto_review\"\n";
    std::fs::write(&config_path, contents)?;
    let mut app_server = start_config_write_test_app_server(&app).await?;
    let started = app_server.start_thread(&app.config).await?;
    let thread_id = started.session.thread_id;
    app.chat_widget
        .handle_thread_session_quiet(started.session.clone());
    app.enqueue_primary_thread_session(started.session, started.turns)
        .await?;
    let contents = std::fs::read_to_string(&config_path)?;
    while events.try_recv().is_ok() {}

    let before = RuntimePermissionProfileOverride::from_config(app.chat_widget.config_ref());
    let before_reviewer = app.config.approvals_reviewer;
    app.apply_permission_shortcut(&mut app_server, thread_id, case.selection)
        .await;

    assert_eq!(
        RuntimePermissionProfileOverride::from_config(app.chat_widget.config_ref()),
        before
    );
    assert_eq!(app.config.approvals_reviewer, before_reviewer);
    assert!(app.pending_server_profiles.contains_key(&thread_id));
    let requested_message = next_history_message(&mut events);

    let second_selection = if expected_profile_id == ":read-only" {
        full_access_selection()
    } else {
        read_only_selection()
    };
    app.apply_permission_shortcut(&mut app_server, thread_id, second_selection)
        .await;
    assert_eq!(
        RuntimePermissionProfileOverride::from_config(app.chat_widget.config_ref()),
        before
    );
    assert_eq!(
        app.pending_server_profiles[&thread_id].profile_id,
        expected_profile_id
    );
    insta::assert_snapshot!(
        next_history_message(&mut events),
        @"■ Wait for permissions to update before changing permissions."
    );

    let notification = next_thread_settings_updated(&mut app_server, thread_id).await;
    let settings = notification.thread_settings.clone();
    app.enqueue_thread_notification(
        thread_id,
        ServerNotification::ThreadSettingsUpdated(notification),
    )
    .await?;
    assert!(!app.pending_server_profiles.contains_key(&thread_id));
    let widget_config = app.chat_widget.config_ref();
    let permissions = &widget_config.permissions;
    assert_eq!(
        (
            settings.approval_policy,
            settings.approvals_reviewer,
            settings.sandbox_policy,
            settings.active_permission_profile,
            app.config.approvals_reviewer,
            widget_config.approvals_reviewer,
            AskForApproval::from(permissions.approval_policy.value()),
            permissions.permission_profile().clone(),
            permissions.active_permission_profile(),
        ),
        (
            expected_approval,
            expected_app_reviewer,
            case.expected_sandbox_policy,
            Some(codex_app_server_protocol::ActivePermissionProfile {
                id: expected_profile_id.clone(),
                extends: None,
            }),
            expected_reviewer,
            expected_reviewer,
            expected_approval,
            case.expected_profile,
            Some(ActivePermissionProfile::new(expected_profile_id)),
        )
    );
    assert_eq!(std::fs::read_to_string(config_path)?, contents);
    assert!(
        events.try_recv().is_err(),
        "must not queue another update or config write"
    );
    app_server.shutdown().await?;
    Ok(requested_message)
}

#[tokio::test]
async fn permission_shortcut_rejections_leave_state_unchanged() -> Result<()> {
    for experimental_api in [false, true] {
        let (mut app, mut events, _op_rx) = make_test_app_with_channels().await;
        let thread_id = ThreadId::new();
        app.active_thread_id = Some(thread_id);
        app.chat_widget
            .handle_thread_session_quiet(test_thread_session(
                thread_id,
                app.config.cwd.to_path_buf(),
            ));
        let original = RuntimePermissionProfileOverride::from_config(app.chat_widget.config_ref());
        let original_reviewer = app.config.approvals_reviewer;
        let client = crate::start_embedded_app_server_with(
            Arg0DispatchPaths::default(),
            app.config.clone(),
            Vec::new(),
            LoaderOverrides::without_managed_config_for_tests(),
            /*strict_config*/ false,
            CloudConfigBundleLoader::default(),
            codex_feedback::CodexFeedback::new(),
            /*log_db*/ None,
            /*state_db*/ None,
            Arc::clone(&app.environment_manager),
            Default::default(),
            |mut args| {
                args.experimental_api = experimental_api;
                codex_app_server_client::InProcessAppServerClient::start(args)
            },
        )
        .await?;
        let mut app_server = AppServerSession::new(
            codex_app_server_client::AppServerClient::InProcess(client),
            crate::app_server_session::ThreadParamsMode::Embedded,
        );
        while events.try_recv().is_ok() {}
        let transcript_len = app.transcript_cells.len();
        app.apply_permission_shortcut(&mut app_server, ThreadId::new(), read_only_selection())
            .await;
        assert_eq!(app.transcript_cells.len(), transcript_len);
        assert!(events.try_recv().is_err());
        app.apply_permission_shortcut(&mut app_server, thread_id, read_only_selection())
            .await;
        assert_eq!(
            RuntimePermissionProfileOverride::from_config(app.chat_widget.config_ref()),
            original
        );
        assert_eq!(app.config.approvals_reviewer, original_reviewer);
        insta::assert_snapshot!(
            if experimental_api {
                "permission_shortcut_server_error"
            } else {
                "permission_shortcut_unsupported"
            },
            next_history_message(&mut events).replace(&thread_id.to_string(), "<THREAD_ID>")
        );
        assert!(events.try_recv().is_err());
        app_server.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn permission_shortcut_confirms_without_persisting() -> Result<()> {
    let history =
        assert_permission_shortcut_confirms_without_persisting(ConfirmedPermissionShortcutCase {
            selection: read_only_selection(),
            expected_profile: PermissionProfile::read_only(),
            expected_sandbox_policy: codex_app_server_protocol::SandboxPolicy::ReadOnly {
                network_access: false,
            },
        })
        .await?;
    insta::assert_snapshot!(history, @"• Permission selection requested: Read Only");
    Ok(())
}

#[tokio::test]
async fn permission_shortcut_full_access_confirms_without_persisting() -> Result<()> {
    let history =
        assert_permission_shortcut_confirms_without_persisting(ConfirmedPermissionShortcutCase {
            selection: full_access_selection(),
            expected_profile: PermissionProfile::Disabled,
            expected_sandbox_policy: codex_app_server_protocol::SandboxPolicy::DangerFullAccess,
        })
        .await?;
    insta::assert_snapshot!(history, @"• Permission selection requested: Full Access");
    Ok(())
}
