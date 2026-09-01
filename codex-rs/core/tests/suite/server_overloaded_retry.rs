use anyhow::Result;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::ErrorEvent;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::turn_input::TurnInputRequest;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::time::Duration;
use wiremock::ResponseTemplate;

fn overloaded_response() -> ResponseTemplate {
    ResponseTemplate::new(503).set_body_json(json!({
        "error": {
            "code": "server_is_overloaded",
            "message": "selected model is at capacity"
        }
    }))
}

async fn submit_user_input(test: &TestCodex, text: &str) -> Result<()> {
    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: text.to_string(),
            text_elements: Vec::new(),
        }]))
        .await?;
    Ok(())
}

#[tokio::test]
async fn capacity_retries_use_a_separate_budget_on_the_same_turn() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_response_sequence(
        &server,
        vec![
            responses::sse_response(responses::sse_failed(
                "resp-stream-error",
                "server_error",
                "temporary stream failure",
            )),
            overloaded_response(),
            responses::sse_response(responses::sse(vec![
                responses::ev_response_created("resp-retried"),
                responses::ev_completed("resp-retried"),
            ])),
        ],
    )
    .await;
    let test = test_codex()
        .with_config(|config| {
            config.model_provider.request_max_retries = Some(0);
            config.model_provider.stream_max_retries = Some(1);
        })
        .build_with_auto_env(&server)
        .await?;
    tokio::time::pause();

    submit_user_input(&test, "keep working").await?;

    let mut generic_retry_messages = Vec::new();
    let mut capacity_retry_messages = Vec::new();
    loop {
        match test.codex.next_event().await?.msg {
            EventMsg::StreamError(event) => {
                if event.codex_error_info == Some(CodexErrorInfo::ServerOverloaded) {
                    capacity_retry_messages.push(event.message);
                } else {
                    generic_retry_messages.push(event.message);
                }
                tokio::time::advance(Duration::from_secs(600)).await;
            }
            EventMsg::Error(error) => anyhow::bail!("capacity retry terminated early: {error:?}"),
            EventMsg::TurnComplete(event) => {
                assert_eq!(event.error, None);
                break;
            }
            _ => {}
        }
    }

    assert_eq!(generic_retry_messages, vec!["Reconnecting... 1/1"]);
    assert_eq!(capacity_retry_messages, vec!["Reconnecting... 1/3"]);
    let requests = response_mock.requests();
    assert_eq!(requests.len(), 3);
    let turn_id = requests[0].body_json()["client_metadata"]["turn_id"].clone();
    assert!(
        requests
            .iter()
            .all(|request| request.body_json()["client_metadata"]["turn_id"] == turn_id)
    );

    Ok(())
}

#[tokio::test]
async fn capacity_exhaustion_emits_three_retries_and_one_terminal_error() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_response_sequence(
        &server,
        vec![
            overloaded_response(),
            overloaded_response(),
            overloaded_response(),
            overloaded_response(),
        ],
    )
    .await;
    let test = test_codex()
        .with_config(|config| {
            config.model_provider.request_max_retries = Some(0);
            config.model_provider.stream_max_retries = Some(0);
        })
        .build_with_auto_env(&server)
        .await?;
    tokio::time::pause();

    submit_user_input(&test, "keep working").await?;

    let mut capacity_retry_messages = Vec::new();
    let mut terminal_errors = Vec::new();
    loop {
        match test.codex.next_event().await?.msg {
            EventMsg::StreamError(event)
                if event.codex_error_info == Some(CodexErrorInfo::ServerOverloaded) =>
            {
                capacity_retry_messages.push(event.message);
                tokio::time::advance(Duration::from_secs(600)).await;
            }
            EventMsg::Error(error) => terminal_errors.push(error),
            EventMsg::TurnComplete(event) => {
                assert_eq!(
                    event.error.and_then(|error| error.codex_error_info),
                    Some(CodexErrorInfo::ServerOverloaded)
                );
                break;
            }
            _ => {}
        }
    }

    assert_eq!(
        capacity_retry_messages,
        vec![
            "Reconnecting... 1/3",
            "Reconnecting... 2/3",
            "Reconnecting... 3/3",
        ]
    );
    assert_eq!(
        terminal_errors,
        vec![ErrorEvent {
            message: "Selected model is at capacity. Please try again later.".to_string(),
            codex_error_info: Some(CodexErrorInfo::ServerOverloaded),
            misalignment: None,
        }]
    );
    assert_eq!(response_mock.requests().len(), 4);

    Ok(())
}

#[tokio::test]
async fn interrupting_capacity_backoff_prevents_another_request() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock =
        responses::mount_response_sequence(&server, vec![overloaded_response()]).await;
    let test = test_codex()
        .with_config(|config| {
            config.model_provider.request_max_retries = Some(0);
            config.model_provider.stream_max_retries = Some(0);
        })
        .build_with_auto_env(&server)
        .await?;
    tokio::time::pause();

    submit_user_input(&test, "keep working").await?;

    loop {
        match test.codex.next_event().await?.msg {
            EventMsg::StreamError(event)
                if event.codex_error_info == Some(CodexErrorInfo::ServerOverloaded) =>
            {
                break;
            }
            EventMsg::Error(error) => anyhow::bail!("capacity retry terminated early: {error:?}"),
            _ => {}
        }
    }

    test.codex.submit(Op::Interrupt).await?;
    loop {
        if matches!(test.codex.next_event().await?.msg, EventMsg::TurnAborted(_)) {
            break;
        }
    }
    assert_eq!(response_mock.requests().len(), 1);

    Ok(())
}
