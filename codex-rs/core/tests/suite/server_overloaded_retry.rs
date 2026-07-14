use anyhow::Result;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::ErrorEvent;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_response_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::sse_failed;
use core_test_support::responses::sse_response;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::time::Duration;
use wiremock::MockServer;
use wiremock::ResponseTemplate;

fn overloaded_response() -> ResponseTemplate {
    ResponseTemplate::new(503).set_body_json(json!({
        "error": {
            "code": "server_is_overloaded",
            "message": "selected model is at capacity"
        }
    }))
}

#[tokio::test]
async fn server_overload_retries_use_a_separate_budget_on_the_same_turn() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let responses = mount_response_sequence(
        &server,
        vec![
            sse_response(sse_failed(
                "resp-stream-error",
                "server_error",
                "temporary stream failure",
            )),
            ResponseTemplate::new(500).set_body_string("temporary gateway failure"),
            overloaded_response(),
            overloaded_response(),
            overloaded_response(),
            sse_response(sse(vec![
                ev_response_created("resp-retried"),
                ev_completed("resp-retried"),
            ])),
        ],
    )
    .await;

    let test = test_codex()
        .with_config(|config| {
            config.model_provider.request_max_retries = Some(4);
            config.model_provider.stream_max_retries = Some(1);
        })
        .build(&server)
        .await?;
    tokio::time::pause();

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "keep working".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;

    let mut server_overload_retry_messages = Vec::new();
    loop {
        match test.codex.next_event().await?.msg {
            EventMsg::StreamError(event) => {
                if event.codex_error_info == Some(CodexErrorInfo::ServerOverloaded) {
                    server_overload_retry_messages.push(event.message);
                }
                tokio::time::advance(Duration::from_secs(600)).await;
            }
            EventMsg::TurnComplete(_) => break,
            _ => {}
        }
    }

    assert_eq!(
        server_overload_retry_messages,
        vec![
            "Reconnecting... 1/3",
            "Reconnecting... 2/3",
            "Reconnecting... 3/3",
        ]
    );
    let requests = responses.requests();
    assert_eq!(requests.len(), 6);
    let turn_id = requests[0].body_json()["client_metadata"]["turn_id"].clone();
    assert!(
        requests
            .iter()
            .all(|request| request.body_json()["client_metadata"]["turn_id"] == turn_id)
    );

    Ok(())
}

#[tokio::test]
async fn server_overload_stops_after_the_capacity_retry_budget() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let responses = mount_response_sequence(
        &server,
        vec![
            overloaded_response(),
            overloaded_response(),
            overloaded_response(),
            overloaded_response(),
        ],
    )
    .await;
    let test = test_codex().build(&server).await?;
    tokio::time::pause();

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "keep working".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;

    let mut server_overload_retry_messages = Vec::new();
    let terminal_error = loop {
        match test.codex.next_event().await?.msg {
            EventMsg::StreamError(event)
                if event.codex_error_info == Some(CodexErrorInfo::ServerOverloaded) =>
            {
                server_overload_retry_messages.push(event.message);
                tokio::time::advance(Duration::from_secs(600)).await;
            }
            EventMsg::Error(error) => break error,
            _ => {}
        }
    };

    assert_eq!(
        server_overload_retry_messages,
        vec![
            "Reconnecting... 1/3",
            "Reconnecting... 2/3",
            "Reconnecting... 3/3",
        ]
    );
    assert_eq!(
        terminal_error,
        ErrorEvent {
            message: "Selected model is at capacity. Please try again later.".to_string(),
            codex_error_info: Some(CodexErrorInfo::ServerOverloaded),
        }
    );
    assert_eq!(responses.requests().len(), 4);

    Ok(())
}

#[tokio::test]
async fn server_overload_backoff_stops_when_the_turn_is_interrupted() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let responses = mount_response_sequence(&server, vec![overloaded_response()]).await;
    let test = test_codex().build(&server).await?;
    tokio::time::pause();

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "keep working".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;

    loop {
        match test.codex.next_event().await?.msg {
            EventMsg::StreamError(event)
                if event.codex_error_info == Some(CodexErrorInfo::ServerOverloaded) =>
            {
                break;
            }
            _ => {}
        }
    }

    test.codex.submit(Op::Interrupt).await?;
    loop {
        if matches!(test.codex.next_event().await?.msg, EventMsg::TurnAborted(_)) {
            break;
        }
    }

    assert_eq!(responses.requests().len(), 1);

    Ok(())
}
