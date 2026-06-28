use super::*;
use crate::test_support::PathBufExt;
#[cfg(windows)]
use crate::workspace_command::AppServerWorkspaceCommandRunner;
use crate::workspace_command::WorkspaceCommandError;
use crate::workspace_command::WorkspaceCommandExecutor;
use crate::workspace_command::WorkspaceCommandOutput;
use crate::workspace_command::WorkspaceCommandPlatform;
#[cfg(windows)]
use codex_app_server_client::AppServerRequestHandle;
#[cfg(windows)]
use codex_app_server_client::DEFAULT_IN_PROCESS_CHANNEL_CAPACITY;
#[cfg(windows)]
use codex_app_server_client::InProcessAppServerClient;
#[cfg(windows)]
use codex_app_server_client::InProcessClientStartArgs;
use codex_app_server_protocol::HookCompletedNotification as AppServerHookCompletedNotification;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadGoal as AppThreadGoal;
use codex_app_server_protocol::ThreadGoalStatus as AppThreadGoalStatus;
#[cfg(windows)]
use codex_arg0::Arg0DispatchPaths;
#[cfg(windows)]
use codex_cloud_config::cloud_config_bundle_loader_for_storage;
#[cfg(windows)]
use codex_config::ConfigBuilder;
#[cfg(windows)]
use codex_config::types::AuthCredentialsStoreMode;
use codex_config::types::CustomStatusLineType;
#[cfg(windows)]
use codex_feedback::CodexFeedback;
#[cfg(windows)]
use codex_login::AuthKeyringBackendKind;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_protocol::models::ActivePermissionProfile;
use codex_protocol::models::BUILT_IN_PERMISSION_PROFILE_DANGER_FULL_ACCESS;
use codex_protocol::models::BUILT_IN_PERMISSION_PROFILE_WORKSPACE;
use codex_protocol::models::PermissionProfile;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use pretty_assertions::assert_eq;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

#[tokio::test]
async fn refresh_event_applies_rendered_status_line() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let runner = FakeStatusLineRunner::new(vec![WorkspaceCommandOutput {
        exit_code: 0,
        stdout: "ASYNC statusline\n".to_string(),
        stderr: String::new(),
    }]);
    let config = CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: allowed_statusline_command(""),
        env: BTreeMap::new(),
        padding: 1,
    };
    let (mut chat, _app_event_tx, mut rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.cwd = tempdir.path().to_path_buf().abs();
    chat.current_cwd = Some(tempdir.path().to_path_buf());
    chat.config.tui_custom_status_line = Some(config);
    chat.workspace_command_runner = Some(runner);
    chat.show_welcome_banner = false;

    chat.refresh_custom_status_line();

    let (request_id, result) = next_custom_status_line_event(&mut rx).await;

    assert_eq!(
        result.as_deref().and_then(first_renderable_line),
        Some("ASYNC statusline")
    );
    chat.apply_custom_status_line_rendered(request_id, result);

    assert!(rendered_custom_status_line(&mut chat).contains("ASYNC statusline"));
}

#[tokio::test]
async fn unchanged_refresh_does_not_spawn_duplicate_command() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let runner = FakeStatusLineRunner::new(vec![
        WorkspaceCommandOutput {
            exit_code: 0,
            stdout: "first statusline\n".to_string(),
            stderr: String::new(),
        },
        WorkspaceCommandOutput {
            exit_code: 0,
            stdout: "second statusline\n".to_string(),
            stderr: String::new(),
        },
    ]);
    let (mut chat, _app_event_tx, mut rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.cwd = tempdir.path().to_path_buf().abs();
    chat.current_cwd = Some(tempdir.path().to_path_buf());
    chat.config.tui_custom_status_line = Some(CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: allowed_statusline_command(""),
        env: BTreeMap::new(),
        padding: 0,
    });
    chat.workspace_command_runner = Some(runner.clone());
    chat.show_welcome_banner = false;

    chat.refresh_custom_status_line();
    let (request_id, result) = next_custom_status_line_event(&mut rx).await;
    chat.apply_custom_status_line_rendered(request_id, result);
    assert!(rendered_custom_status_line(&mut chat).contains("first statusline"));

    chat.refresh_custom_status_line();
    tokio::time::sleep(Duration::from_millis(20)).await;

    assert!(rendered_custom_status_line(&mut chat).contains("first statusline"));
    assert_eq!(runner.commands().len(), 1);
}

#[tokio::test]
async fn stop_hook_completion_forces_refresh_for_external_status_state() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let runner = FakeStatusLineRunner::new(vec![
        WorkspaceCommandOutput {
            exit_code: 0,
            stdout: "working statusline\n".to_string(),
            stderr: String::new(),
        },
        WorkspaceCommandOutput {
            exit_code: 0,
            stdout: "idle statusline\n".to_string(),
            stderr: String::new(),
        },
    ]);
    let (mut chat, _app_event_tx, mut rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.cwd = tempdir.path().to_path_buf().abs();
    chat.current_cwd = Some(tempdir.path().to_path_buf());
    chat.config.tui_custom_status_line = Some(CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: allowed_statusline_command(""),
        env: BTreeMap::new(),
        padding: 0,
    });
    chat.workspace_command_runner = Some(runner.clone());
    chat.show_welcome_banner = false;

    chat.refresh_custom_status_line();
    let (request_id, result) = next_custom_status_line_event(&mut rx).await;
    chat.apply_custom_status_line_rendered(request_id, result);
    assert!(rendered_custom_status_line(&mut chat).contains("working statusline"));

    chat.handle_server_notification(
        ServerNotification::HookCompleted(AppServerHookCompletedNotification {
            thread_id: "thread".to_string(),
            turn_id: Some("turn".to_string()),
            run: codex_app_server_protocol::HookRunSummary {
                id: "stop-hook".to_string(),
                event_name: codex_app_server_protocol::HookEventName::Stop,
                handler_type: codex_app_server_protocol::HookHandlerType::Command,
                execution_mode: codex_app_server_protocol::HookExecutionMode::Sync,
                scope: codex_app_server_protocol::HookScope::Turn,
                source_path: tempdir.path().join("hooks.json").abs(),
                source: codex_app_server_protocol::HookSource::User,
                display_order: 0,
                status: codex_app_server_protocol::HookRunStatus::Completed,
                status_message: Some("completed".to_string()),
                started_at: 1,
                completed_at: Some(2),
                duration_ms: Some(1),
                entries: Vec::new(),
            },
        }),
        /*replay_kind*/ None,
    );
    let (request_id, result) = next_custom_status_line_event(&mut rx).await;
    chat.apply_custom_status_line_rendered(request_id, result);

    assert!(rendered_custom_status_line(&mut chat).contains("idle statusline"));
    assert_eq!(runner.commands().len(), 2);
}

#[tokio::test]
async fn terminal_resize_refresh_uses_new_width() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let runner = FakeStatusLineRunner::new(vec![WorkspaceCommandOutput {
        exit_code: 0,
        stdout: "resized statusline\n".to_string(),
        stderr: String::new(),
    }]);
    let (mut chat, _app_event_tx, mut rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.cwd = tempdir.path().to_path_buf().abs();
    chat.current_cwd = Some(tempdir.path().to_path_buf());
    chat.config.tui_custom_status_line = Some(CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: allowed_statusline_command(""),
        env: BTreeMap::new(),
        padding: 0,
    });
    chat.workspace_command_runner = Some(runner.clone());

    chat.on_terminal_resize(/*width*/ 132);

    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("custom status line event should arrive")
        .expect("event channel should remain open");
    assert!(matches!(
        event,
        AppEvent::CustomStatusLineRendered {
            result: Some(_),
            ..
        }
    ));
    let commands = runner.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(
        commands[0].env.get("COLUMNS"),
        Some(&Some("132".to_string()))
    );
}

#[tokio::test]
async fn terminal_resize_after_initial_render_refreshes_for_new_width() {
    let runner = FakeStatusLineRunner::new(vec![WorkspaceCommandOutput {
        exit_code: 0,
        stdout: "resized\n".to_string(),
        stderr: String::new(),
    }]);
    let (mut chat, _app_event_tx, mut rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.tui_custom_status_line = Some(CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: allowed_statusline_command(""),
        env: BTreeMap::new(),
        padding: 0,
    });
    chat.workspace_command_runner = Some(runner.clone());
    chat.show_welcome_banner = false;

    chat.custom_status_line_state.next_request_id = 1;
    chat.custom_status_line_state.pending_request_id = Some(1);
    chat.apply_custom_status_line_rendered(1, Some("previous status\n".to_string()));
    assert!(rendered_custom_status_line(&mut chat).contains("previous status"));

    chat.last_rendered_width.set(Some(80));
    chat.on_terminal_resize(/*width*/ 120);

    assert!(rendered_custom_status_line(&mut chat).contains("previous status"));
    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("custom status line event should arrive")
        .expect("event channel should remain open");
    assert!(matches!(
        event,
        AppEvent::CustomStatusLineRendered {
            result: Some(_),
            ..
        }
    ));
    let commands = runner.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(
        commands[0].env.get("COLUMNS"),
        Some(&Some("120".to_string()))
    );
}

#[tokio::test]
async fn pending_refresh_spawns_latest_queued_request_after_current_result() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let runner = FakeStatusLineRunner::new(vec![WorkspaceCommandOutput {
        exit_code: 0,
        stdout: "queued status\n".to_string(),
        stderr: String::new(),
    }]);
    let (mut chat, _app_event_tx, mut rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.cwd = tempdir.path().to_path_buf().abs();
    chat.current_cwd = Some(tempdir.path().to_path_buf());
    chat.config.tui_custom_status_line = Some(CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: allowed_statusline_command(""),
        env: BTreeMap::new(),
        padding: 0,
    });
    chat.workspace_command_runner = Some(runner.clone());
    chat.show_welcome_banner = false;
    chat.custom_status_line_state.next_request_id = 1;
    chat.custom_status_line_state.pending_request_id = Some(1);
    chat.custom_status_line_state.pending = Some(tokio::spawn(std::future::pending()));

    chat.refresh_custom_status_line();

    assert!(runner.commands().is_empty());
    chat.apply_custom_status_line_rendered(1, Some("stale status\n".to_string()));
    assert!(!rendered_custom_status_line(&mut chat).contains("stale status"));

    let (request_id, result) = next_custom_status_line_event(&mut rx).await;
    chat.apply_custom_status_line_rendered(request_id, result);

    assert!(rendered_custom_status_line(&mut chat).contains("queued status"));
    let commands = runner.commands();
    assert_eq!(commands.len(), 1);
}

#[tokio::test]
async fn queued_refresh_preserves_payload_snapshot() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let runner = FakeStatusLineRunner::new(vec![WorkspaceCommandOutput {
        exit_code: 0,
        stdout: "queued status\n".to_string(),
        stderr: String::new(),
    }]);
    let (mut chat, _app_event_tx, mut rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.cwd = tempdir.path().to_path_buf().abs();
    chat.current_cwd = Some(tempdir.path().to_path_buf());
    chat.config.tui_custom_status_line = Some(CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: allowed_statusline_command(""),
        env: BTreeMap::new(),
        padding: 0,
    });
    chat.workspace_command_runner = Some(runner.clone());
    chat.show_welcome_banner = false;
    chat.custom_status_line_state.next_request_id = 1;
    chat.custom_status_line_state.pending_request_id = Some(1);
    chat.custom_status_line_state.pending = Some(tokio::spawn(std::future::pending()));

    chat.apply_runtime_metrics_delta(codex_otel::RuntimeMetricsSummary {
        api_calls: codex_otel::RuntimeMetricTotals {
            count: 1,
            duration_ms: 321,
        },
        ..codex_otel::RuntimeMetricsSummary::default()
    });
    chat.refresh_custom_status_line();
    chat.turn_runtime_metrics = codex_otel::RuntimeMetricsSummary::default();

    chat.apply_custom_status_line_rendered(1, Some("stale status\n".to_string()));
    let (request_id, result) = next_custom_status_line_event(&mut rx).await;
    chat.apply_custom_status_line_rendered(request_id, result);

    let commands = runner.commands();
    assert_eq!(commands.len(), 1);
    let payload: Value = serde_json::from_slice(
        commands[0]
            .stdin
            .as_deref()
            .expect("Unix statusline command should receive JSON stdin"),
    )
    .expect("payload should be valid JSON");
    assert_eq!(
        payload.pointer("/cost/total_duration_ms"),
        Some(&json!(321))
    );
}

#[tokio::test]
async fn payload_includes_yolo_permissions_snapshot() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let (mut chat, _app_event_tx, _rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.cwd = tempdir.path().to_path_buf().abs();
    chat.current_cwd = Some(tempdir.path().to_path_buf());
    chat.set_approval_policy(AskForApproval::Never);
    chat.set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
        PermissionProfile::Disabled,
        ActivePermissionProfile::new(BUILT_IN_PERMISSION_PROFILE_DANGER_FULL_ACCESS),
    ))
    .expect("full-access permission profile should be accepted");

    let payload = chat.custom_status_line_payload(tempdir.path());

    assert_eq!(
        payload.get("permissions"),
        Some(&json!({
            "mode": "yolo",
            "label": "YOLO",
            "approval_policy": "never",
            "approvals_reviewer": "user",
            "active_profile_id": BUILT_IN_PERMISSION_PROFILE_DANGER_FULL_ACCESS,
            "active_profile_extends": null,
            "file_system": "unrestricted",
            "network": "enabled",
            "enforcement": "disabled",
            "yolo": true,
        }))
    );
}

#[tokio::test]
async fn payload_includes_effective_reasoning_effort() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let (mut chat, _app_event_tx, _rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.cwd = tempdir.path().to_path_buf().abs();
    chat.current_cwd = Some(tempdir.path().to_path_buf());

    for (effort, expected) in [
        (Some(ReasoningEffortConfig::Low), Some("low")),
        (Some(ReasoningEffortConfig::High), Some("high")),
        (Some(ReasoningEffortConfig::XHigh), Some("xhigh")),
        (None, None),
    ] {
        chat.set_reasoning_effort(effort);
        let payload = chat.custom_status_line_payload(tempdir.path());
        assert_eq!(
            payload.pointer("/effort/level").cloned(),
            expected.map(|value| json!(value))
        );
    }

    chat.set_plan_mode_reasoning_effort(Some(ReasoningEffortConfig::High));
    let plan_mask = collaboration_modes::plan_mask(chat.model_catalog.as_ref())
        .expect("expected plan collaboration mode");
    chat.set_collaboration_mask(plan_mask);
    let payload = chat.custom_status_line_payload(tempdir.path());
    assert_eq!(payload.pointer("/effort/level"), Some(&json!("high")));
}

#[tokio::test]
async fn payload_includes_named_permissions_profile_identity() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let (mut chat, _app_event_tx, _rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.cwd = tempdir.path().to_path_buf().abs();
    chat.current_cwd = Some(tempdir.path().to_path_buf());
    chat.set_approval_policy(AskForApproval::OnRequest);
    chat.set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
        PermissionProfile::workspace_write(),
        ActivePermissionProfile {
            id: "locked-down".to_string(),
            extends: Some(BUILT_IN_PERMISSION_PROFILE_WORKSPACE.to_string()),
        },
    ))
    .expect("named permission profile should be accepted");

    let payload = chat.custom_status_line_payload(tempdir.path());

    assert_eq!(
        payload.get("permissions"),
        Some(&json!({
            "mode": "custom",
            "label": "locked-down",
            "approval_policy": "on-request",
            "approvals_reviewer": "user",
            "active_profile_id": "locked-down",
            "active_profile_extends": BUILT_IN_PERMISSION_PROFILE_WORKSPACE,
            "file_system": "restricted",
            "network": "restricted",
            "enforcement": "managed",
            "yolo": false,
        }))
    );
}

#[tokio::test]
async fn payload_includes_auto_permissions_mode() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let (mut chat, _app_event_tx, _rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.cwd = tempdir.path().to_path_buf().abs();
    chat.current_cwd = Some(tempdir.path().to_path_buf());
    chat.set_approval_policy(AskForApproval::OnRequest);
    chat.set_approvals_reviewer(ApprovalsReviewer::AutoReview);
    chat.set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
        PermissionProfile::workspace_write(),
        ActivePermissionProfile::new(BUILT_IN_PERMISSION_PROFILE_WORKSPACE),
    ))
    .expect("workspace permission profile should be accepted");

    let payload = chat.custom_status_line_payload(tempdir.path());

    assert_eq!(payload.pointer("/permissions/mode"), Some(&json!("auto")));
    assert_eq!(payload.pointer("/permissions/label"), Some(&json!("Auto")));
    assert_eq!(
        payload.pointer("/permissions/approvals_reviewer"),
        Some(&json!("auto_review"))
    );
    assert_eq!(
        payload.pointer("/permissions/approval_policy"),
        Some(&json!("on-request"))
    );
}

#[tokio::test]
async fn permissions_update_refreshes_custom_status_line_payload() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let runner = FakeStatusLineRunner::new(vec![
        WorkspaceCommandOutput {
            exit_code: 0,
            stdout: "workspace status\n".to_string(),
            stderr: String::new(),
        },
        WorkspaceCommandOutput {
            exit_code: 0,
            stdout: "yolo status\n".to_string(),
            stderr: String::new(),
        },
    ]);
    let (mut chat, _app_event_tx, mut rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.cwd = tempdir.path().to_path_buf().abs();
    chat.current_cwd = Some(tempdir.path().to_path_buf());
    chat.config.tui_custom_status_line = Some(CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: allowed_statusline_command(""),
        env: BTreeMap::new(),
        padding: 0,
    });
    chat.config
        .permissions
        .approval_policy
        .set(AskForApproval::Never.to_core())
        .expect("approval policy should be accepted");
    chat.workspace_command_runner = Some(runner.clone());
    chat.show_welcome_banner = false;

    chat.refresh_custom_status_line();
    let (request_id, result) = next_custom_status_line_event(&mut rx).await;
    chat.apply_custom_status_line_rendered(request_id, result);

    chat.set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
        PermissionProfile::Disabled,
        ActivePermissionProfile::new(BUILT_IN_PERMISSION_PROFILE_DANGER_FULL_ACCESS),
    ))
    .expect("full-access permission profile should be accepted");
    let (request_id, result) = next_custom_status_line_event(&mut rx).await;
    chat.apply_custom_status_line_rendered(request_id, result);

    let commands = runner.commands();
    assert_eq!(commands.len(), 2);
    let payload: Value = serde_json::from_slice(
        commands[1]
            .stdin
            .as_deref()
            .expect("Unix statusline command should receive JSON stdin"),
    )
    .expect("payload should be valid JSON");
    assert_eq!(payload.pointer("/permissions/yolo"), Some(&json!(true)));
    assert_eq!(
        payload.pointer("/permissions/active_profile_id"),
        Some(&json!(BUILT_IN_PERMISSION_PROFILE_DANGER_FULL_ACCESS))
    );
}

#[tokio::test]
async fn approvals_reviewer_update_refreshes_custom_status_line_payload() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let runner = FakeStatusLineRunner::new(vec![
        WorkspaceCommandOutput {
            exit_code: 0,
            stdout: "ask status\n".to_string(),
            stderr: String::new(),
        },
        WorkspaceCommandOutput {
            exit_code: 0,
            stdout: "auto status\n".to_string(),
            stderr: String::new(),
        },
    ]);
    let (mut chat, _app_event_tx, mut rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.cwd = tempdir.path().to_path_buf().abs();
    chat.current_cwd = Some(tempdir.path().to_path_buf());
    chat.config.tui_custom_status_line = Some(CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: allowed_statusline_command(""),
        env: BTreeMap::new(),
        padding: 0,
    });
    chat.set_approval_policy(AskForApproval::OnRequest);
    chat.set_approvals_reviewer(ApprovalsReviewer::User);
    chat.set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
        PermissionProfile::workspace_write(),
        ActivePermissionProfile::new(BUILT_IN_PERMISSION_PROFILE_WORKSPACE),
    ))
    .expect("workspace permission profile should be accepted");
    chat.workspace_command_runner = Some(runner.clone());
    chat.show_welcome_banner = false;

    chat.refresh_custom_status_line();
    let (request_id, result) = next_custom_status_line_event(&mut rx).await;
    chat.apply_custom_status_line_rendered(request_id, result);

    chat.set_approvals_reviewer(ApprovalsReviewer::AutoReview);
    let (request_id, result) = next_custom_status_line_event(&mut rx).await;
    chat.apply_custom_status_line_rendered(request_id, result);

    let commands = runner.commands();
    assert_eq!(commands.len(), 2);
    let payload: Value = serde_json::from_slice(
        commands[1]
            .stdin
            .as_deref()
            .expect("Unix statusline command should receive JSON stdin"),
    )
    .expect("payload should be valid JSON");
    assert_eq!(payload.pointer("/permissions/mode"), Some(&json!("auto")));
    assert_eq!(payload.pointer("/permissions/label"), Some(&json!("Auto")));
    assert_eq!(
        payload.pointer("/permissions/approvals_reviewer"),
        Some(&json!("auto_review"))
    );
}

#[tokio::test]
async fn command_failure_hides_status_line() {
    let runner = FakeStatusLineRunner::new(vec![WorkspaceCommandOutput {
        exit_code: 1,
        stdout: "ignored\n".to_string(),
        stderr: "failed\n".to_string(),
    }]);
    let config = CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: "statusline-command".to_string(),
        env: BTreeMap::new(),
        padding: 0,
    };

    let output = render_custom_status_line_command(
        config,
        std::env::current_dir().expect("cwd should be available"),
        json!({}),
        /*columns*/ 80,
        runner,
        WorkspaceCommandPlatform::Unix,
    )
    .await;

    assert_eq!(output, None);
}

#[tokio::test]
async fn failed_refresh_can_retry_unchanged_snapshot() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let runner = FakeStatusLineRunner::new(vec![
        WorkspaceCommandOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "temporary failure\n".to_string(),
        },
        WorkspaceCommandOutput {
            exit_code: 0,
            stdout: "recovered statusline\n".to_string(),
            stderr: String::new(),
        },
    ]);
    let (mut chat, _app_event_tx, mut rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.cwd = tempdir.path().to_path_buf().abs();
    chat.current_cwd = Some(tempdir.path().to_path_buf());
    chat.config.tui_custom_status_line = Some(CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: allowed_statusline_command(""),
        env: BTreeMap::new(),
        padding: 0,
    });
    chat.workspace_command_runner = Some(runner.clone());
    chat.show_welcome_banner = false;

    chat.refresh_custom_status_line();
    let (request_id, result) = next_custom_status_line_event(&mut rx).await;
    chat.apply_custom_status_line_rendered(request_id, result);
    assert!(!rendered_custom_status_line(&mut chat).contains("temporary failure"));

    chat.refresh_custom_status_line();
    let (request_id, result) = next_custom_status_line_event(&mut rx).await;
    chat.apply_custom_status_line_rendered(request_id, result);

    assert!(rendered_custom_status_line(&mut chat).contains("recovered statusline"));
    assert_eq!(runner.commands().len(), 2);
}

#[test]
fn first_renderable_line_skips_ansi_only_lines() {
    assert_eq!(
        first_renderable_line("\u{1b}[31m\nvisible\n"),
        Some("visible")
    );
    assert_eq!(first_renderable_line("\u{1b}[31m\u{1b}[0m\n"), None);
}

#[tokio::test]
async fn failed_refresh_clears_existing_status_line() {
    let (mut chat, _app_event_tx, _rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.tui_custom_status_line = Some(CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: "statusline-command".to_string(),
        env: BTreeMap::new(),
        padding: 0,
    });
    chat.show_welcome_banner = false;

    chat.custom_status_line_state.next_request_id = 1;
    chat.custom_status_line_state.pending_request_id = Some(1);
    chat.apply_custom_status_line_rendered(1, Some("previous status\n".to_string()));
    assert!(rendered_custom_status_line(&mut chat).contains("previous status"));

    chat.custom_status_line_state.next_request_id = 2;
    chat.custom_status_line_state.pending_request_id = Some(2);
    chat.apply_custom_status_line_rendered(2, None);
    assert!(!rendered_custom_status_line(&mut chat).contains("previous status"));
}

#[tokio::test]
async fn failed_refresh_restores_builtin_status_line() {
    let (mut chat, _app_event_tx, _rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.tui_custom_status_line = Some(CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: "statusline-command".to_string(),
        env: BTreeMap::new(),
        padding: 0,
    });
    chat.show_welcome_banner = false;
    chat.bottom_pane.set_status_line_enabled(/*enabled*/ true);
    chat.bottom_pane
        .set_status_line(Some(Line::from("built-in status")));

    chat.custom_status_line_state.next_request_id = 1;
    chat.custom_status_line_state.pending_request_id = Some(1);
    chat.apply_custom_status_line_rendered(1, Some("custom status\n".to_string()));
    let rendered = rendered_custom_status_line(&mut chat);
    assert!(rendered.contains("custom status"));
    assert!(!rendered.contains("built-in status"));

    chat.custom_status_line_state.next_request_id = 2;
    chat.custom_status_line_state.pending_request_id = Some(2);
    chat.apply_custom_status_line_rendered(2, None);
    let rendered = rendered_custom_status_line(&mut chat);
    assert!(!rendered.contains("custom status"));
    assert!(rendered.contains("built-in status"));
}

#[tokio::test]
async fn payload_includes_bounded_goal_snapshot() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let (mut chat, _app_event_tx, _rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.cwd = tempdir.path().to_path_buf().abs();
    chat.current_cwd = Some(tempdir.path().to_path_buf());
    chat.current_goal_status = Some(GoalStatusState::new(
        AppThreadGoal {
            thread_id: "thread".to_string(),
            objective: "x".repeat(CUSTOM_STATUS_LINE_GOAL_OBJECTIVE_MAX_CHARS + 20),
            status: AppThreadGoalStatus::Active,
            token_budget: Some(50_000),
            tokens_used: 12_500,
            time_used_seconds: 120,
            created_at: 1,
            updated_at: 2,
        },
        std::time::Instant::now(),
    ));

    let payload = chat.custom_status_line_payload(tempdir.path());
    let goal = payload
        .get("goal")
        .and_then(Value::as_object)
        .expect("goal payload should be an object");

    let objective = goal
        .get("objective")
        .and_then(Value::as_str)
        .expect("objective should be present");
    assert_eq!(
        objective.chars().count(),
        CUSTOM_STATUS_LINE_GOAL_OBJECTIVE_MAX_CHARS
    );
    assert!(objective.ends_with("..."));
    assert_eq!(goal.get("status"), Some(&json!("active")));
    assert_eq!(goal.get("token_budget"), Some(&json!(50_000)));
    assert_eq!(goal.get("tokens_used"), Some(&json!(12_500)));
    assert_eq!(goal.get("time_used_seconds"), Some(&json!(120)));
}

#[tokio::test]
async fn goal_update_refreshes_custom_status_line_payload() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let runner = FakeStatusLineRunner::new(vec![
        WorkspaceCommandOutput {
            exit_code: 0,
            stdout: "initial status\n".to_string(),
            stderr: String::new(),
        },
        WorkspaceCommandOutput {
            exit_code: 0,
            stdout: "goal status\n".to_string(),
            stderr: String::new(),
        },
    ]);
    let (mut chat, _app_event_tx, mut rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    chat.config.cwd = tempdir.path().to_path_buf().abs();
    chat.current_cwd = Some(tempdir.path().to_path_buf());
    chat.config.tui_custom_status_line = Some(CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: allowed_statusline_command(""),
        env: BTreeMap::new(),
        padding: 0,
    });
    chat.workspace_command_runner = Some(runner.clone());
    chat.show_welcome_banner = false;

    chat.refresh_custom_status_line();
    let (request_id, result) = next_custom_status_line_event(&mut rx).await;
    chat.apply_custom_status_line_rendered(request_id, result);

    chat.on_thread_goal_updated(
        AppThreadGoal {
            thread_id: "thread".to_string(),
            objective: "Ship statusline".to_string(),
            status: AppThreadGoalStatus::Active,
            token_budget: Some(10_000),
            tokens_used: 200,
            time_used_seconds: 30,
            created_at: 1,
            updated_at: 2,
        },
        None,
    );
    let (request_id, result) = next_custom_status_line_event(&mut rx).await;
    chat.apply_custom_status_line_rendered(request_id, result);

    let commands = runner.commands();
    assert_eq!(commands.len(), 2);
    let payload: Value = serde_json::from_slice(
        commands[1]
            .stdin
            .as_deref()
            .expect("Unix statusline command should receive JSON stdin"),
    )
    .expect("payload should be valid JSON");
    assert_eq!(
        payload.pointer("/goal/objective"),
        Some(&json!("Ship statusline"))
    );
}

#[tokio::test]
async fn completion_refresh_payload_includes_runtime_duration() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let runner = FakeStatusLineRunner::new(vec![
        WorkspaceCommandOutput {
            exit_code: 0,
            stdout: "running status\n".to_string(),
            stderr: String::new(),
        },
        WorkspaceCommandOutput {
            exit_code: 0,
            stdout: "done status\n".to_string(),
            stderr: String::new(),
        },
    ]);
    let (mut chat, _app_event_tx, mut rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    chat.config.cwd = tempdir.path().to_path_buf().abs();
    chat.current_cwd = Some(tempdir.path().to_path_buf());
    chat.config.tui_custom_status_line = Some(CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: allowed_statusline_command(""),
        env: BTreeMap::new(),
        padding: 0,
    });
    chat.workspace_command_runner = Some(runner.clone());
    chat.show_welcome_banner = false;

    chat.on_task_started();
    let (request_id, result) = next_custom_status_line_event(&mut rx).await;
    chat.apply_custom_status_line_rendered(request_id, result);

    chat.apply_runtime_metrics_delta(codex_otel::RuntimeMetricsSummary {
        api_calls: codex_otel::RuntimeMetricTotals {
            count: 1,
            duration_ms: 125,
        },
        tool_calls: codex_otel::RuntimeMetricTotals {
            count: 1,
            duration_ms: 75,
        },
        ..codex_otel::RuntimeMetricsSummary::default()
    });
    chat.on_task_complete(
        /*last_agent_message*/ None, /*duration_ms*/ None, /*from_replay*/ false,
    );
    let (request_id, result) = next_custom_status_line_event(&mut rx).await;
    chat.apply_custom_status_line_rendered(request_id, result);

    let commands = runner.commands();
    assert_eq!(commands.len(), 2);
    let payload: Value = serde_json::from_slice(
        commands[1]
            .stdin
            .as_deref()
            .expect("Unix statusline command should receive JSON stdin"),
    )
    .expect("payload should be valid JSON");
    assert_eq!(
        payload.pointer("/cost/total_duration_ms"),
        Some(&json!(200))
    );
    assert_eq!(chat.custom_status_line_total_duration_ms(), None);
}

#[cfg(windows)]
#[tokio::test]
async fn windows_app_server_runner_pipes_base64_payload_to_command() -> anyhow::Result<()> {
    let codex_home = tempfile::TempDir::new()?;
    let codex_home_path = codex_home.path().to_path_buf();
    let config = ConfigBuilder::default()
        .codex_home(codex_home_path.clone())
        .build()
        .await?;
    let client = InProcessAppServerClient::start(InProcessClientStartArgs {
        arg0_paths: Arg0DispatchPaths::default(),
        config: Arc::new(config),
        cli_overrides: Vec::new(),
        loader_overrides: Default::default(),
        strict_config: false,
        cloud_config_bundle: cloud_config_bundle_loader_for_storage(
            codex_home_path,
            /*enable_codex_api_key_env*/ false,
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
            "https://chatgpt.com/backend-api/".to_string(),
            /*auth_route_config*/ None,
        )
        .await,
        feedback: CodexFeedback::new(),
        log_db: None,
        state_db: None,
        environment_manager: Arc::new(
            codex_app_server_client::EnvironmentManager::default_for_tests(),
        ),
        config_warnings: Vec::new(),
        session_source: serde_json::from_value(serde_json::json!("cli"))?,
        enable_codex_api_key_env: false,
        client_name: "test".to_string(),
        client_version: "test".to_string(),
        experimental_api: true,
        mcp_server_openai_form_elicitation: false,
        opt_out_notification_methods: Vec::new(),
        channel_capacity: DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
    })
    .await?;
    let runner = Arc::new(AppServerWorkspaceCommandRunner::new(
        AppServerRequestHandle::InProcess(client.request_handle()),
        Some("windows"),
    ));
    let cwd = tempfile::TempDir::new()?;
    let config = CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: r"C:\Windows\System32\findstr.exe .*".to_string(),
        env: BTreeMap::new(),
        padding: 0,
    };

    let output = render_custom_status_line_command(
        config,
        cwd.path().to_path_buf(),
        json!({"workspace": {"current_dir": "C:\\project"}}),
        /*columns*/ 120,
        runner,
        WorkspaceCommandPlatform::Windows,
    )
    .await;

    assert_eq!(
        output.as_deref().and_then(first_renderable_line),
        Some(r#"{"workspace":{"current_dir":"C:\\project"}}"#)
    );
    Ok(())
}

fn rendered_custom_status_line(chat: &mut ChatWidget) -> String {
    let width = 80;
    let height = chat.desired_height(width);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))
        .expect("terminal should be created");
    terminal
        .draw(|f| chat.render(f.area(), f.buffer_mut()))
        .expect("chat widget should render");
    format!("{}", terminal.backend())
}

async fn next_custom_status_line_event(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
) -> (u64, Option<String>) {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("custom status line event should arrive")
            .expect("event channel should remain open");
        if let AppEvent::CustomStatusLineRendered { request_id, result } = event {
            return (request_id, result);
        }
    }
}

#[cfg(windows)]
fn allowed_statusline_command(args: &str) -> String {
    format!(r#"C:\codex-statusline.exe{args}"#)
}

#[cfg(not(windows))]
fn allowed_statusline_command(args: &str) -> String {
    format!("statusline-command{args}")
}

struct FakeStatusLineRunner {
    commands: Mutex<Vec<WorkspaceCommand>>,
    outputs: Mutex<VecDeque<WorkspaceCommandOutput>>,
    platform: WorkspaceCommandPlatform,
}

impl FakeStatusLineRunner {
    fn new(outputs: Vec<WorkspaceCommandOutput>) -> Arc<Self> {
        Arc::new(Self {
            commands: Mutex::new(Vec::new()),
            outputs: Mutex::new(outputs.into()),
            platform: WorkspaceCommandPlatform::Unix,
        })
    }

    fn commands(&self) -> Vec<WorkspaceCommand> {
        self.commands
            .lock()
            .expect("commands mutex should not be poisoned")
            .clone()
    }
}

impl WorkspaceCommandExecutor for FakeStatusLineRunner {
    fn platform(&self) -> WorkspaceCommandPlatform {
        self.platform
    }

    fn run(
        &self,
        command: WorkspaceCommand,
    ) -> Pin<
        Box<dyn Future<Output = Result<WorkspaceCommandOutput, WorkspaceCommandError>> + Send + '_>,
    > {
        self.commands
            .lock()
            .expect("commands mutex should not be poisoned")
            .push(command);
        let output = self
            .outputs
            .lock()
            .expect("outputs mutex should not be poisoned")
            .pop_front()
            .expect("fake runner output should be available");
        Box::pin(async move { Ok(output) })
    }
}
