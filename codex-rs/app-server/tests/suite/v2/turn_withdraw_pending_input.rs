use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::create_command_execution_sse_response;
use app_test_support::create_mock_responses_server_sequence_unchecked;
use app_test_support::write_mock_responses_config_toml_with_chatgpt_base_url;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ItemStartedNotification;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnSteerParams;
use codex_app_server_protocol::TurnSteerResponse;
use codex_app_server_protocol::UserInput;
use codex_protocol::ThreadId;
use core_test_support::skip_if_remote;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

const READ_TIMEOUT: Duration = Duration::from_secs(10);

fn sleep_command(seconds: u32) -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        vec![
            "powershell".to_string(),
            "-Command".to_string(),
            format!("Start-Sleep -Seconds {seconds}"),
        ]
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec!["sleep".to_string(), seconds.to_string()]
    }
}

async fn withdraw_error(app: &mut TestAppServer, params: Value) -> Result<JSONRPCErrorError> {
    let request_id = app
        .send_raw_request("turn/withdrawPendingInput", Some(params))
        .await?;
    Ok(timeout(
        READ_TIMEOUT,
        app.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??
    .error)
}

fn withdraw_params(
    thread_id: impl Into<String>,
    expected_turn_id: impl Into<String>,
    client_user_message_id: impl Into<String>,
) -> Value {
    json!({
        "threadId": thread_id.into(),
        "expectedTurnId": expected_turn_id.into(),
        "clientUserMessageId": client_user_message_id.into(),
    })
}

#[tokio::test]
async fn turn_withdraw_pending_input_validates_params_in_order() -> Result<()> {
    let codex_home = TempDir::new()?;
    let responses = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    write_mock_responses_config_toml_with_chatgpt_base_url(
        codex_home.path(),
        &responses.uri(),
        &responses.uri(),
    )?;
    let mut app = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build_initialized()
        .await?;

    let unloaded_thread_id = ThreadId::new().to_string();
    let cases = [
        (
            withdraw_params("", "", ""),
            "threadId must not be empty".to_string(),
        ),
        (
            withdraw_params("not-empty", "", ""),
            "expectedTurnId must not be empty".to_string(),
        ),
        (
            withdraw_params("not-empty", "not-empty", ""),
            "clientUserMessageId must not be empty".to_string(),
        ),
        (
            withdraw_params("not-a-thread-id", "turn-1", "message-1"),
            "invalid thread id".to_string(),
        ),
        (
            withdraw_params(&unloaded_thread_id, "turn-1", "message-1"),
            format!("thread not found: {unloaded_thread_id}"),
        ),
    ];
    for (params, message) in cases {
        assert_eq!(
            withdraw_error(&mut app, params).await?,
            JSONRPCErrorError {
                code: -32600,
                message,
                data: None,
            }
        );
    }

    let ThreadStartResponse { thread, .. } = app
        .start_thread(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    assert_eq!(
        withdraw_error(
            &mut app,
            withdraw_params(&thread.id, "expected-turn", "message-1")
        )
        .await?,
        JSONRPCErrorError {
            code: -32600,
            message: "no active turn contains pending input".to_string(),
            data: Some(json!({
                "reason": "noActiveTurn",
                "expectedTurnId": "expected-turn",
                "actualTurnId": null,
            })),
        }
    );
    Ok(())
}

#[tokio::test]
async fn turn_withdraw_pending_input_maps_active_turn_outcomes() -> Result<()> {
    skip_if_remote!(
        Ok(()),
        "uses a host-local command and cwd fixture unavailable to remote executors"
    );

    let codex_home = TempDir::new()?;
    let working_directory = TempDir::new()?;
    let responses = create_mock_responses_server_sequence_unchecked(vec![
        create_command_execution_sse_response(
            sleep_command(/*seconds*/ 30),
            Some(working_directory.path()),
            Some(30_000),
            "call_sleep",
        )?,
    ])
    .await;
    write_mock_responses_config_toml_with_chatgpt_base_url(
        codex_home.path(),
        &responses.uri(),
        &responses.uri(),
    )?;
    let mut app = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build_initialized()
        .await?;

    let ThreadStartResponse { thread, .. } = app
        .start_thread(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let TurnStartResponse { turn } = app
        .request(|request_id| ClientRequest::TurnStart {
            request_id,
            params: TurnStartParams {
                thread_id: thread.id.clone(),
                input: vec![UserInput::Text {
                    text: "run a command".to_string(),
                    text_elements: Vec::new(),
                }],
                cwd: Some(working_directory.path().to_path_buf()),
                ..Default::default()
            },
        })
        .await?;
    let _: JSONRPCNotification = timeout(
        READ_TIMEOUT,
        app.read_stream_until_notification_message("turn/started"),
    )
    .await??;

    assert_eq!(
        withdraw_error(
            &mut app,
            withdraw_params(&thread.id, "wrong-turn", "missing-message")
        )
        .await?,
        JSONRPCErrorError {
            code: -32600,
            message: format!(
                "expected active turn id `wrong-turn` but found `{}`",
                turn.id
            ),
            data: Some(json!({
                "reason": "expectedTurnMismatch",
                "expectedTurnId": "wrong-turn",
                "actualTurnId": turn.id,
            })),
        }
    );
    assert_eq!(
        withdraw_error(
            &mut app,
            withdraw_params(&thread.id, &turn.id, "missing-message")
        )
        .await?,
        JSONRPCErrorError {
            code: -32600,
            message: "client user message id is not pending".to_string(),
            data: Some(json!({
                "reason": "notPending",
                "expectedTurnId": turn.id,
                "actualTurnId": turn.id,
            })),
        }
    );

    for text in ["first duplicate", "second duplicate"] {
        let response: TurnSteerResponse = app
            .request(|request_id| ClientRequest::TurnSteer {
                request_id,
                params: TurnSteerParams {
                    thread_id: thread.id.clone(),
                    client_user_message_id: Some("duplicate-message".to_string()),
                    input: vec![UserInput::Text {
                        text: text.to_string(),
                        text_elements: Vec::new(),
                    }],
                    responsesapi_client_metadata: None,
                    additional_context: None,
                    expected_turn_id: turn.id.clone(),
                },
            })
            .await?;
        assert_eq!(
            response,
            TurnSteerResponse {
                turn_id: turn.id.clone()
            }
        );
    }
    assert_eq!(
        withdraw_error(
            &mut app,
            withdraw_params(&thread.id, &turn.id, "duplicate-message")
        )
        .await?,
        JSONRPCErrorError {
            code: -32600,
            message: "client user message id matches multiple pending inputs".to_string(),
            data: Some(json!({
                "reason": "ambiguousClientUserMessageId",
                "expectedTurnId": turn.id,
                "actualTurnId": turn.id,
            })),
        }
    );

    let response: TurnSteerResponse = app
        .request(|request_id| ClientRequest::TurnSteer {
            request_id,
            params: TurnSteerParams {
                thread_id: thread.id.clone(),
                client_user_message_id: Some("unique-message".to_string()),
                input: vec![UserInput::Text {
                    text: "withdraw me".to_string(),
                    text_elements: Vec::new(),
                }],
                responsesapi_client_metadata: None,
                additional_context: None,
                expected_turn_id: turn.id.clone(),
            },
        })
        .await?;
    assert_eq!(
        response,
        TurnSteerResponse {
            turn_id: turn.id.clone()
        }
    );
    let request_id = app
        .send_raw_request(
            "turn/withdrawPendingInput",
            Some(withdraw_params(&thread.id, &turn.id, "unique-message")),
        )
        .await?;
    let response = timeout(
        READ_TIMEOUT,
        app.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(response.result, json!({"turnId": turn.id}));

    app.interrupt_turn_and_wait_for_aborted(thread.id, turn.id, READ_TIMEOUT)
        .await?;
    Ok(())
}

#[tokio::test]
async fn turn_withdraw_pending_input_rejects_after_pending_input_is_drained() -> Result<()> {
    skip_if_remote!(
        Ok(()),
        "uses a host-local command and cwd fixture unavailable to remote executors"
    );

    let codex_home = TempDir::new()?;
    let working_directory = TempDir::new()?;
    let responses = create_mock_responses_server_sequence_unchecked(vec![
        create_command_execution_sse_response(
            sleep_command(/*seconds*/ 1),
            Some(working_directory.path()),
            Some(10_000),
            "call_first_sleep",
        )?,
        create_command_execution_sse_response(
            sleep_command(/*seconds*/ 30),
            Some(working_directory.path()),
            Some(30_000),
            "call_second_sleep",
        )?,
    ])
    .await;
    write_mock_responses_config_toml_with_chatgpt_base_url(
        codex_home.path(),
        &responses.uri(),
        &responses.uri(),
    )?;
    let mut app = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build_initialized()
        .await?;

    let ThreadStartResponse { thread, .. } = app
        .start_thread(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let TurnStartResponse { turn } = app
        .request(|request_id| ClientRequest::TurnStart {
            request_id,
            params: TurnStartParams {
                thread_id: thread.id.clone(),
                input: vec![UserInput::Text {
                    text: "run two commands".to_string(),
                    text_elements: Vec::new(),
                }],
                cwd: Some(working_directory.path().to_path_buf()),
                ..Default::default()
            },
        })
        .await?;
    let _: JSONRPCNotification = timeout(
        READ_TIMEOUT,
        app.read_stream_until_notification_message("turn/started"),
    )
    .await??;

    let response: TurnSteerResponse = app
        .request(|request_id| ClientRequest::TurnSteer {
            request_id,
            params: TurnSteerParams {
                thread_id: thread.id.clone(),
                client_user_message_id: Some("drained-message".to_string()),
                input: vec![UserInput::Text {
                    text: "this reaches the next sampling request".to_string(),
                    text_elements: Vec::new(),
                }],
                responsesapi_client_metadata: None,
                additional_context: None,
                expected_turn_id: turn.id.clone(),
            },
        })
        .await?;
    assert_eq!(
        response,
        TurnSteerResponse {
            turn_id: turn.id.clone()
        }
    );

    timeout(READ_TIMEOUT, async {
        loop {
            let notification: ItemStartedNotification =
                app.read_notification("item/started").await?;
            if matches!(
                notification.item,
                ThreadItem::UserMessage { client_id: Some(ref client_id), .. }
                    if client_id == "drained-message"
            ) {
                return Ok::<(), anyhow::Error>(());
            }
        }
    })
    .await??;

    assert_eq!(
        withdraw_error(
            &mut app,
            withdraw_params(&thread.id, &turn.id, "drained-message")
        )
        .await?,
        JSONRPCErrorError {
            code: -32600,
            message: "client user message id is not pending".to_string(),
            data: Some(json!({
                "reason": "notPending",
                "expectedTurnId": turn.id,
                "actualTurnId": turn.id,
            })),
        }
    );

    app.interrupt_turn_and_wait_for_aborted(thread.id, turn.id, READ_TIMEOUT)
        .await?;
    Ok(())
}
