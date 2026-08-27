use std::collections::HashMap;

use codex_protocol::protocol::HookEventName;
use codex_protocol::protocol::HookRunStatus;
use codex_protocol::protocol::HookSource;
use codex_utils_absolute_path::test_support::PathBufExt;
use codex_utils_absolute_path::test_support::test_path_buf;
use pretty_assertions::assert_eq;

use super::NotificationRequest;
use super::NotificationType;
use super::parse_completed;
use super::preview;
use crate::engine::ConfiguredHandler;
use crate::engine::ConfiguredHandlerKind;
use crate::engine::HandlerRunResult;

#[test]
fn notification_matcher_selects_notification_type() {
    let request = NotificationRequest {
        session_id: codex_protocol::ThreadId::new(),
        turn_id: "turn-1".to_string(),
        cwd: test_path_buf("/tmp").abs(),
        transcript_path: None,
        model: "gpt-test".to_string(),
        permission_mode: "default".to_string(),
        notification_type: NotificationType::UserInputRequest,
    };

    let selected = preview(
        &[
            handler(Some("elicitation_dialog"), /*display_order*/ 0),
            handler(Some("user_input_request"), /*display_order*/ 1),
            handler(/*matcher*/ None, /*display_order*/ 2),
        ],
        &request,
    );

    assert_eq!(
        selected
            .into_iter()
            .map(|run| run.display_order)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[test]
fn notification_ignores_successful_control_output() {
    let completed = parse_completed(
        &handler(/*matcher*/ None, /*display_order*/ 0),
        HandlerRunResult {
            started_at: 1,
            completed_at: 2,
            duration_ms: 1,
            exit_code: Some(0),
            stdout: r#"{"continue":false,"decision":"block","reason":"ignored"}"#.to_string(),
            stderr: String::new(),
            error: None,
        },
        Some("turn-1".to_string()),
    );

    assert_eq!(completed.completed.run.status, HookRunStatus::Completed);
    assert_eq!(completed.completed.run.entries, Vec::new());
}

#[test]
fn notification_reports_failure_without_control_effects() {
    let completed = parse_completed(
        &handler(/*matcher*/ None, /*display_order*/ 0),
        HandlerRunResult {
            started_at: 1,
            completed_at: 2,
            duration_ms: 1,
            exit_code: Some(7),
            stdout: String::new(),
            stderr: "observer failed".to_string(),
            error: None,
        },
        Some("turn-1".to_string()),
    );

    assert_eq!(completed.completed.run.status, HookRunStatus::Failed);
    assert_eq!(completed.completed.run.entries[0].text, "observer failed");
}

fn handler(matcher: Option<&str>, display_order: i64) -> ConfiguredHandler {
    ConfiguredHandler {
        event_name: HookEventName::Notification,
        matcher: matcher.map(str::to_string),
        timeout_sec: 2,
        status_message: None,
        additional_context_limit: Default::default(),
        source_path: test_path_buf("/tmp/hooks.json").abs().into(),
        source: HookSource::User,
        display_order,
        kind: ConfiguredHandlerKind::Command {
            command: "echo hook".to_string(),
            r#async: false,
            env: HashMap::new(),
        },
    }
}
