#![allow(clippy::unwrap_used)]

use codex_core::TurnInputRequest;
use core_test_support::test_codex::local_selections;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use codex_features::Feature;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Settings;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ThreadSettingsOverrides;
use codex_protocol::request_user_input::RequestUserInputAnswer;
use codex_protocol::request_user_input::RequestUserInputResponse;
use codex_protocol::user_input::UserInput;
use core_test_support::TempDirExt;
use core_test_support::responses;
use core_test_support::responses::ResponsesRequest;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_completed_with_tokens;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::test_codex::turn_permission_fields;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_match;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tokio::time::Duration;
use tokio::time::timeout;

#[derive(Clone, Copy)]
enum NotificationHookMode {
    Sync,
    Async,
}

fn write_user_input_notification_hook(
    home: &Path,
    mode: NotificationHookMode,
) -> anyhow::Result<()> {
    let script_path = home.join("user_input_notification_hook.py");
    let log_path = home.join("user_input_notification_hook.jsonl");
    let script = format!(
        r#"import json
from pathlib import Path
import sys

payload = json.load(sys.stdin)
with Path(r"{log_path}").open("a", encoding="utf-8") as handle:
    handle.write(json.dumps(payload) + "\n")
raise SystemExit(7)
"#,
        log_path = log_path.display(),
    );
    let hooks = json!({
        "hooks": {
            "Notification": [{
                "matcher": "user_input_request|user_input_complete",
                "hooks": [{
                    "type": "command",
                    "command": format!("python3 {}", script_path.display()),
                    "async": matches!(mode, NotificationHookMode::Async),
                }]
            }]
        }
    });

    fs::write(script_path, script)?;
    fs::write(home.join("hooks.json"), hooks.to_string())?;
    Ok(())
}

async fn wait_for_notification_count(log_path: &Path, count: usize) -> anyhow::Result<()> {
    timeout(Duration::from_secs(5), async {
        loop {
            let payloads = fs::read_to_string(log_path).unwrap_or_default();
            if payloads.lines().count() >= count {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

fn call_output(req: &ResponsesRequest, call_id: &str) -> String {
    let raw = req.function_call_output(call_id);
    assert_eq!(
        raw.get("call_id").and_then(Value::as_str),
        Some(call_id),
        "mismatched call_id in function_call_output"
    );
    let (content_opt, _success) = req
        .function_call_output_content_and_success(call_id)
        .expect("function_call_output present");
    content_opt.expect("function_call_output content present")
}

fn call_output_content_and_success(
    req: &ResponsesRequest,
    call_id: &str,
) -> (String, Option<bool>) {
    let raw = req.function_call_output(call_id);
    assert_eq!(
        raw.get("call_id").and_then(Value::as_str),
        Some(call_id),
        "mismatched call_id in function_call_output"
    );
    let (content_opt, success) = req
        .function_call_output_content_and_success(call_id)
        .expect("function_call_output present");
    let content = content_opt.expect("function_call_output content present");
    (content, success)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_request_user_input_emits_paired_attention_notifications() -> anyhow::Result<()> {
    request_user_input_round_trip_for_mode(ModeKind::Plan).await
}

async fn request_user_input_round_trip_for_mode(mode: ModeKind) -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let hook_mode = if mode == ModeKind::Plan {
        NotificationHookMode::Sync
    } else {
        NotificationHookMode::Async
    };
    let mut builder = test_codex()
        .with_pre_build_hook(move |home| {
            write_user_input_notification_hook(home, hook_mode)
                .expect("write user-input notification hook fixture");
        })
        .with_config(move |config| {
            config
                .features
                .enable(Feature::CodexHooks)
                .expect("test config should allow feature update");
            config.bypass_hook_trust = true;
            if mode == ModeKind::Default {
                config
                    .features
                    .enable(Feature::DefaultModeRequestUserInput)
                    .expect("test config should allow feature update");
            }
        });
    let TestCodex {
        codex,
        cwd,
        home,
        session_configured,
        ..
    } = builder.build_with_auto_env(&server).await?;

    let call_id = "user-input-call";
    let expected_is_blocking = mode == ModeKind::Plan;
    let request_args = json!({
        "questions": [{
            "id": "confirm_path",
            "header": "Confirm",
            "question": "Proceed with the plan?",
            "options": [{
                "label": "Yes (Recommended)",
                "description": "Continue the current plan."
            }, {
                "label": "No",
                "description": "Stop and revisit the approach."
            }]
        }]
    });
    let request_args = request_args.to_string();

    let first_response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(call_id, "request_user_input", &request_args),
        ev_rate_limits(),
        ev_completed("resp-1"),
    ]);
    responses::mount_sse_once(&server, first_response).await;

    let second_response = sse(vec![
        ev_assistant_message("msg-1", "thanks"),
        ev_completed("resp-2"),
    ]);
    let second_mock = responses::mount_sse_once(&server, second_response).await;

    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd.path());

    codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: "please confirm".into(),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                environments: Some(local_selections(cwd.abs())),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(CollaborationMode {
                    mode,
                    settings: Settings {
                        model: session_configured.model.clone(),
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            }),
        )
        .await?;

    let request = wait_for_event_match(&codex, |event| match event {
        EventMsg::RequestUserInput(request) => Some(request.clone()),
        _ => None,
    })
    .await;
    assert_eq!(request.call_id, call_id);
    assert_eq!(request.questions.len(), 1);
    assert_eq!(request.is_blocking, expected_is_blocking);
    assert_eq!(request.auto_resolution_ms, None);
    assert_eq!(request.questions[0].is_other, true);
    let log_path = home.path().join("user_input_notification_hook.jsonl");
    wait_for_notification_count(&log_path, /*count*/ 1).await?;
    let open_payload: Value = serde_json::from_str(
        fs::read_to_string(&log_path)?
            .lines()
            .next()
            .expect("open notification log line"),
    )?;
    assert_eq!(open_payload["notification_type"], "user_input_request");
    assert_eq!(open_payload["message"], "Codex needs your input");
    assert_eq!(open_payload["hook_event_name"], "Notification");
    assert_eq!(open_payload["permission_mode"], "bypassPermissions");
    assert_eq!(open_payload.get("questions"), None);
    assert_eq!(open_payload.get("answers"), None);
    assert!(
        timeout(Duration::from_millis(200), async {
            loop {
                let event = codex
                    .next_event()
                    .await
                    .expect("event stream should stay open");
                if matches!(event.msg, EventMsg::TokenCount(_)) {
                    return;
                }
            }
        })
        .await
        .is_err(),
        "TokenCount should wait until request_user_input resolves"
    );

    let answers = if mode == ModeKind::Plan {
        HashMap::from([(
            "confirm_path".to_string(),
            RequestUserInputAnswer {
                answers: vec!["yes".to_string()],
            },
        )])
    } else {
        HashMap::new()
    };
    let response = RequestUserInputResponse { answers };
    codex
        .submit(Op::UserInputAnswer {
            id: request.turn_id.clone(),
            response,
        })
        .await?;

    wait_for_event(&codex, |event| matches!(event, EventMsg::TokenCount(_))).await;
    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;
    wait_for_notification_count(&log_path, /*count*/ 2).await?;

    let req = second_mock.single_request();
    let output_text = call_output(&req, call_id);
    let output_json: Value = serde_json::from_str(&output_text)?;
    let expected_output = if mode == ModeKind::Plan {
        json!({"answers": {"confirm_path": {"answers": ["yes"]}}})
    } else {
        json!({"answers": {}})
    };
    assert_eq!(output_json, expected_output);
    let payloads = fs::read_to_string(log_path)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(payloads.len(), 2);
    assert_eq!(payloads[1]["notification_type"], "user_input_complete");
    assert_eq!(payloads[1]["message"], "Codex input request completed");
    assert_eq!(payloads[1].get("questions"), None);
    assert_eq!(payloads[1].get("answers"), None);

    Ok(())
}

fn ev_rate_limits() -> Value {
    json!({
        "type": "codex.rate_limits",
        "plan_type": "plus",
        "rate_limits": {
            "allowed": true,
            "limit_reached": false,
            "primary": {
                "used_percent": 42,
                "window_minutes": 60,
                "reset_at": 1700000000
            },
            "secondary": null
        },
        "code_review_rate_limits": null,
        "credits": null,
        "promo": null
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_user_input_interrupt_emits_deferred_token_count() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut builder = test_codex()
        .with_pre_build_hook(|home| {
            write_user_input_notification_hook(home, NotificationHookMode::Sync)
                .expect("write user-input notification hook fixture");
        })
        .with_config(|config| {
            config
                .features
                .enable(Feature::CodexHooks)
                .expect("test config should allow feature update");
            config.bypass_hook_trust = true;
        });
    let TestCodex {
        codex,
        cwd,
        home,
        session_configured,
        ..
    } = builder.build_with_auto_env(&server).await?;

    let call_id = "user-input-interrupt";
    let request_args = json!({
        "questions": [{
            "id": "confirm_path",
            "header": "Confirm",
            "question": "Proceed with the plan?",
            "options": [{
                "label": "Yes (Recommended)",
                "description": "Continue the current plan."
            }, {
                "label": "No",
                "description": "Stop and revisit the approach."
            }]
        }]
    })
    .to_string();

    let response = sse(vec![
        ev_response_created("resp-interrupt"),
        ev_function_call(call_id, "request_user_input", &request_args),
        ev_completed_with_tokens("resp-interrupt", /*total_tokens*/ 77),
    ]);
    responses::mount_sse_once(&server, response).await;

    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd.path());
    codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: "please confirm".into(),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                environments: Some(local_selections(cwd.abs())),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Plan,
                    settings: Settings {
                        model: session_configured.model,
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            }),
        )
        .await?;

    let request = wait_for_event_match(&codex, |event| match event {
        EventMsg::RequestUserInput(request) => Some(request.clone()),
        _ => None,
    })
    .await;
    let log_path = home.path().join("user_input_notification_hook.jsonl");
    wait_for_notification_count(&log_path, /*count*/ 1).await?;

    codex.submit(Op::Interrupt).await?;

    let token_count = wait_for_event_match(&codex, |event| match event {
        EventMsg::TokenCount(token_count) => Some(token_count.clone()),
        _ => None,
    })
    .await;
    assert_eq!(
        token_count
            .info
            .map(|info| info.total_token_usage.total_tokens),
        Some(77)
    );
    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnAborted(_))).await;

    wait_for_notification_count(&log_path, /*count*/ 2).await?;
    let notification_types = fs::read_to_string(log_path)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|payload| payload["notification_type"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        notification_types,
        vec![json!("user_input_request"), json!("user_input_complete")]
    );

    assert_eq!(request.call_id, call_id);
    Ok(())
}

async fn assert_request_user_input_rejected<F>(mode_name: &str, build_mode: F) -> anyhow::Result<()>
where
    F: FnOnce(String) -> CollaborationMode,
{
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let mut builder = test_codex();
    let TestCodex {
        codex,
        cwd,
        session_configured,
        ..
    } = builder.build(&server).await?;

    let mode_slug = mode_name.to_lowercase().replace(' ', "-");
    let call_id = format!("user-input-{mode_slug}-call");
    let request_args = json!({
        "questions": [{
            "id": "confirm_path",
            "header": "Confirm",
            "question": "Proceed with the plan?",
            "options": [{
                "label": "Yes (Recommended)",
                "description": "Continue the current plan."
            }, {
                "label": "No",
                "description": "Stop and revisit the approach."
            }]
        }]
    })
    .to_string();

    let first_response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(&call_id, "request_user_input", &request_args),
        ev_completed("resp-1"),
    ]);
    responses::mount_sse_once(&server, first_response).await;

    let second_response = sse(vec![
        ev_assistant_message("msg-1", "thanks"),
        ev_completed("resp-2"),
    ]);
    let second_mock = responses::mount_sse_once(&server, second_response).await;

    let session_model = session_configured.model.clone();
    let collaboration_mode = build_mode(session_model.clone());
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd.path());

    codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: "please confirm".into(),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                environments: Some(local_selections(cwd.abs())),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(collaboration_mode),
                ..Default::default()
            }),
        )
        .await?;

    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let req = second_mock.single_request();
    let (output, success) = call_output_content_and_success(&req, &call_id);
    assert_eq!(success, None);
    assert_eq!(
        output,
        format!("request_user_input is unavailable in {mode_name} mode")
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_user_input_rejected_in_default_mode_by_default() -> anyhow::Result<()> {
    assert_request_user_input_rejected("Default", |model| CollaborationMode {
        mode: ModeKind::Default,
        settings: Settings {
            model,
            reasoning_effort: None,
            developer_instructions: None,
        },
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_blocking_auto_resolution_response_emits_paired_attention_notifications()
-> anyhow::Result<()> {
    request_user_input_round_trip_for_mode(ModeKind::Default).await
}
