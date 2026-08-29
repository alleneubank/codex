use std::collections::VecDeque;
use std::sync::Mutex;

use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCResponse;
use futures::SinkExt;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use super::*;
use crate::chatwidget::PendingSteerLifecycle;
use crate::chatwidget::UserMessage;

#[derive(Debug)]
enum Reply {
    Result(serde_json::Value),
    Error(JSONRPCErrorError),
    Disconnect,
}

#[derive(Debug)]
struct Step {
    method: &'static str,
    reply: Reply,
}

impl Step {
    fn result(method: &'static str, result: serde_json::Value) -> Self {
        Self {
            method,
            reply: Reply::Result(result),
        }
    }

    fn error(method: &'static str, message: &str, data: Option<serde_json::Value>) -> Self {
        Self {
            method,
            reply: Reply::Error(JSONRPCErrorError {
                code: -32602,
                message: message.to_string(),
                data,
            }),
        }
    }
}

type ClientIds = Arc<Mutex<Vec<String>>>;
type AppEvents = tokio::sync::mpsc::UnboundedReceiver<AppEvent>;
type PendingFixture = (App, AppEvents, Op, ThreadId, String, String);

fn pending_lifecycle(app: &App, client_id: &str) -> Option<PendingSteerLifecycle> {
    app.chat_widget
        .capture_thread_input_state()?
        .pending_steers
        .into_iter()
        .find(|pending| pending.client_user_message_id == client_id)
        .map(|pending| pending.lifecycle)
}

fn inserted_history(events: &mut AppEvents) -> Vec<String> {
    std::iter::from_fn(|| events.try_recv().ok())
        .filter_map(|event| match event {
            AppEvent::InsertHistoryCell(cell) => {
                Some(lines_to_single_string(&cell.display_lines(/*width*/ 80)))
            }
            _ => None,
        })
        .collect()
}

async fn start_scripted_app_server(
    config: &Config,
    steps: Vec<Step>,
) -> Result<(AppServerSession, ClientIds, JoinHandle<Result<()>>)> {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_sink = Arc::clone(&requests);
    let mut steps = VecDeque::from(steps);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let websocket_url = format!("ws://{}", listener.local_addr()?);
    let codex_home = config.codex_home.display().to_string();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut websocket = accept_async(stream).await?;
        while let Some(frame) = websocket.next().await {
            let Message::Text(text) = frame? else {
                continue;
            };
            match serde_json::from_str::<JSONRPCMessage>(&text)? {
                JSONRPCMessage::Request(request) if request.method == "initialize" => {
                    websocket
                        .send(Message::Text(
                            serde_json::to_string(&JSONRPCMessage::Response(JSONRPCResponse {
                                id: request.id,
                                result: serde_json::json!({
                                    "userAgent": "codex-tui-test",
                                    "codexHome": codex_home,
                                }),
                            }))?
                            .into(),
                        ))
                        .await?;
                }
                JSONRPCMessage::Request(request) => {
                    let step = steps.pop_front().expect("unexpected scripted request");
                    assert_eq!(request.method, step.method);
                    let client_id = request.params.as_ref().unwrap()["clientUserMessageId"]
                        .as_str()
                        .unwrap()
                        .to_string();
                    request_sink
                        .lock()
                        .expect("request recorder lock")
                        .push(client_id);
                    let message = match step.reply {
                        Reply::Result(result) => JSONRPCMessage::Response(JSONRPCResponse {
                            id: request.id,
                            result,
                        }),
                        Reply::Error(error) => JSONRPCMessage::Error(JSONRPCError {
                            id: request.id,
                            error,
                        }),
                        Reply::Disconnect => break,
                    };
                    websocket
                        .send(Message::Text(serde_json::to_string(&message)?.into()))
                        .await?;
                }
                JSONRPCMessage::Notification(_)
                | JSONRPCMessage::Response(_)
                | JSONRPCMessage::Error(_) => {}
            }
        }
        assert!(
            steps.is_empty(),
            "scripted responses were not consumed: {steps:?}"
        );
        Ok(())
    });
    let client = crate::connect_remote_app_server(crate::RemoteAppServerEndpoint::WebSocket {
        websocket_url,
        auth_token: None,
    })
    .await?;
    Ok((
        AppServerSession::new(
            client,
            crate::app_server_session::ThreadParamsMode::Embedded,
        )
        .with_startup_config(config),
        requests,
        server,
    ))
}

async fn pending_submission_fixture() -> Result<PendingFixture> {
    let (mut app, mut app_event_rx, mut op_rx) = make_test_app_with_channels().await;
    let thread_id = ThreadId::new();
    app.enqueue_primary_thread_session(
        test_thread_session(thread_id, app.config.cwd.to_path_buf()),
        vec![test_turn(
            "turn-initial",
            TurnStatus::InProgress,
            Vec::new(),
        )],
    )
    .await?;
    while app_event_rx.try_recv().is_ok() {}
    while op_rx.try_recv().is_ok() {}
    app.chat_widget
        .restore_user_message_to_composer(UserMessage::from("pending steer"));
    app.chat_widget
        .handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let op = next_user_turn_op(&mut op_rx);
    let client_user_message_id = match &op {
        Op::UserTurn {
            client_user_message_id,
            ..
        } => client_user_message_id.clone(),
        _ => unreachable!(),
    };
    assert_eq!(
        pending_lifecycle(&app, &client_user_message_id),
        Some(PendingSteerLifecycle::AwaitingAcceptance)
    );
    app.chat_widget
        .restore_user_message_to_composer(UserMessage::from("second pending steer"));
    app.chat_widget
        .handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let second_client_user_message_id = match next_user_turn_op(&mut op_rx) {
        Op::UserTurn {
            client_user_message_id,
            ..
        } => client_user_message_id,
        _ => unreachable!(),
    };
    while app_event_rx.try_recv().is_ok() {}
    Ok((
        app,
        app_event_rx,
        op,
        thread_id,
        client_user_message_id,
        second_client_user_message_id,
    ))
}

#[tokio::test]
async fn routing_accepts_steer_retry_and_start_fallback_with_one_stable_id() -> Result<()> {
    let cases = [
        (
            vec![
                Step::error(
                    "turn/steer",
                    "expected active turn id `turn-initial` but found `turn-retry`",
                    None,
                ),
                Step::result("turn/steer", serde_json::json!({"turnId": "turn-response"})),
            ],
            PendingSteerLifecycle::Accepted {
                turn_id: "turn-response".to_string(),
            },
        ),
        (
            vec![
                Step::error("turn/steer", "no active turn to steer", None),
                Step::result(
                    "turn/start",
                    serde_json::json!({"turn": test_turn("turn-started", TurnStatus::InProgress, Vec::new())}),
                ),
            ],
            PendingSteerLifecycle::AwaitingCommitAfterStart {
                turn_id: "turn-started".to_string(),
            },
        ),
    ];

    for (steps, expected_lifecycle) in cases {
        let (mut app, _events, op, thread_id, client_id, _second_client_id) =
            pending_submission_fixture().await?;
        let (mut app_server, requests, server) =
            start_scripted_app_server(&app.config, steps).await?;

        assert!(
            app.try_submit_active_thread_op_via_app_server(&mut app_server, thread_id, &op)
                .await?
        );
        assert_eq!(
            pending_lifecycle(&app, &client_id),
            Some(expected_lifecycle)
        );
        assert!(requests.lock().unwrap().iter().all(|id| id == &client_id));
        app_server.shutdown().await?;
        server.await??;
    }
    Ok(())
}

#[tokio::test]
async fn definitive_rejections_recover_exact_row_and_route_source_error() -> Result<()> {
    let turn_error = AppServerTurnError {
        message: "cannot steer a review turn".to_string(),
        codex_error_info: Some(AppServerCodexErrorInfo::ActiveTurnNotSteerable {
            turn_kind: AppServerNonSteerableTurnKind::Review,
        }),
        additional_details: None,
        misalignment: None,
    };
    let cases = [
        (
            vec![Step::error(
                "turn/steer",
                &turn_error.message,
                Some(serde_json::to_value(&turn_error)?),
            )],
            "cannot steer a review turn",
        ),
        (
            vec![Step::error("turn/steer", "generic steer rejection", None)],
            "generic steer rejection",
        ),
        (
            vec![
                Step::error("turn/steer", "no active turn to steer", None),
                Step::error("turn/start", "start rejected", None),
            ],
            "start rejected",
        ),
    ];

    for (steps, error_fragment) in cases {
        let (mut app, mut events, op, thread_id, client_id, second_client_id) =
            pending_submission_fixture().await?;
        let (mut app_server, _requests, server) =
            start_scripted_app_server(&app.config, steps).await?;

        assert!(app.chat_widget.is_agent_turn_running());
        assert!(
            app.try_submit_active_thread_op_via_app_server(&mut app_server, thread_id, &op)
                .await?
        );
        let diagnostic = app.active_thread_rx.as_mut().unwrap().try_recv()?;
        assert!(matches!(diagnostic, ThreadBufferedEvent::LocalError(_)));
        app.handle_thread_event_now(diagnostic);
        assert_eq!(pending_lifecycle(&app, &client_id), None);
        assert_eq!(
            pending_lifecycle(&app, &second_client_id),
            Some(PendingSteerLifecycle::AwaitingAcceptance)
        );
        assert!(app.chat_widget.is_agent_turn_running());
        let errors = inserted_history(&mut events);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains(error_fragment));
        app_server.shutdown().await?;
        server.await??;
    }
    Ok(())
}

#[tokio::test]
async fn uncertain_submission_is_retained_and_warned_once() -> Result<()> {
    for reply in [
        Reply::Result(serde_json::json!({"wrong": true})),
        Reply::Disconnect,
    ] {
        let (mut app, _events, op, thread_id, client_id, _second_client_id) =
            pending_submission_fixture().await?;
        let (mut app_server, _requests, server) = start_scripted_app_server(
            &app.config,
            vec![Step {
                method: "turn/steer",
                reply,
            }],
        )
        .await?;

        assert!(
            app.try_submit_active_thread_op_via_app_server(&mut app_server, thread_id, &op)
                .await?
        );
        assert_eq!(
            pending_lifecycle(&app, &client_id),
            Some(PendingSteerLifecycle::AcceptanceUncertain)
        );
        assert!(matches!(
            app.active_thread_rx.as_mut().unwrap().try_recv(),
            Ok(ThreadBufferedEvent::Notification(notification))
                if matches!(notification.as_ref(), ServerNotification::Warning(_))
        ));
        assert!(app.active_thread_rx.as_mut().unwrap().try_recv().is_err());
        let _ = app_server.shutdown().await;
        server.await??;
    }
    Ok(())
}

#[tokio::test]
async fn commit_before_response_makes_late_routing_success_a_noop() -> Result<()> {
    let (mut app, _events, op, thread_id, client_id, _second_client_id) =
        pending_submission_fixture().await?;
    app.handle_thread_event_now(ThreadBufferedEvent::Notification(Box::new(
        ServerNotification::ItemCompleted(codex_app_server_protocol::ItemCompletedNotification {
            thread_id: thread_id.to_string(),
            turn_id: "turn-initial".to_string(),
            completed_at_ms: 0,
            item: ThreadItem::UserMessage {
                id: "user-committed".to_string(),
                client_id: Some(client_id.clone()),
                content: vec![codex_app_server_protocol::UserInput::Text {
                    text: "pending steer".to_string(),
                    text_elements: Vec::new(),
                }],
            },
        }),
    )));
    let committed_state = app.chat_widget.capture_thread_input_state();
    assert_eq!(pending_lifecycle(&app, &client_id), None);
    let (mut app_server, _requests, server) = start_scripted_app_server(
        &app.config,
        vec![Step::result(
            "turn/steer",
            serde_json::json!({"turnId": "turn-late-response"}),
        )],
    )
    .await?;

    assert!(
        app.try_submit_active_thread_op_via_app_server(&mut app_server, thread_id, &op)
            .await?
    );
    assert_eq!(
        app.chat_widget.capture_thread_input_state(),
        committed_state
    );
    assert!(app.active_thread_rx.as_mut().unwrap().try_recv().is_err());
    app_server.shutdown().await?;
    server.await??;
    Ok(())
}

#[tokio::test]
async fn offscreen_response_updates_source_store_not_displayed_widget() -> Result<()> {
    let (mut app, _events, op, source_thread_id, client_id, _second_client_id) =
        pending_submission_fixture().await?;
    let source_state = app.chat_widget.capture_thread_input_state().unwrap();
    let source_store = Arc::clone(&app.thread_event_channels[&source_thread_id].store);
    {
        let mut store = source_store.lock().await;
        store.active = false;
        store.input_state = Some(source_state);
    }
    app.active_thread_id = Some(ThreadId::new());
    let (mut app_server, _requests, server) = start_scripted_app_server(
        &app.config,
        vec![Step::result(
            "turn/steer",
            serde_json::json!({"turnId": "turn-offscreen"}),
        )],
    )
    .await?;

    assert!(
        app.try_submit_active_thread_op_via_app_server(&mut app_server, source_thread_id, &op)
            .await?
    );
    assert_eq!(
        source_store
            .lock()
            .await
            .input_state
            .as_ref()
            .unwrap()
            .pending_steers[0]
            .lifecycle,
        PendingSteerLifecycle::Accepted {
            turn_id: "turn-offscreen".to_string()
        }
    );
    assert_eq!(
        pending_lifecycle(&app, &client_id),
        Some(PendingSteerLifecycle::AwaitingAcceptance)
    );
    app_server.shutdown().await?;
    server.await??;
    Ok(())
}

#[tokio::test]
async fn offscreen_rejection_recovers_source_row_and_buffers_source_error() -> Result<()> {
    let (mut app, mut events, op, source_thread_id, client_id, second_client_id) =
        pending_submission_fixture().await?;
    let source_state = app.chat_widget.capture_thread_input_state().unwrap();
    let mut expected_source_state = source_state.clone();
    assert_eq!(
        expected_source_state.reconcile_pending_steer_submission(
            &client_id,
            crate::chatwidget::PendingSteerSubmissionOutcome::DefinitivelyRejected,
        ),
        crate::chatwidget::PendingSteerSubmissionEffect::Rejected
    );
    let source_store = Arc::clone(&app.thread_event_channels[&source_thread_id].store);
    {
        let mut store = source_store.lock().await;
        store.active = false;
        store.input_state = Some(source_state);
    }
    app.active_thread_id = Some(ThreadId::new());
    let displayed_state = app.chat_widget.capture_thread_input_state();
    let (mut app_server, _requests, server) = start_scripted_app_server(
        &app.config,
        vec![Step::error("turn/steer", "offscreen steer rejected", None)],
    )
    .await?;

    assert!(
        app.try_submit_active_thread_op_via_app_server(&mut app_server, source_thread_id, &op)
            .await?
    );
    let (replay_state, diagnostic) = {
        let mut store = source_store.lock().await;
        assert_eq!(store.input_state, Some(expected_source_state));
        let diagnostic = store.buffer.pop_back().expect("buffered local error");
        assert!(matches!(
            &diagnostic,
            ThreadBufferedEvent::LocalError(message)
                if message.contains("offscreen steer rejected")
        ));
        (store.input_state.clone(), diagnostic)
    };
    assert_eq!(
        app.chat_widget.capture_thread_input_state(),
        displayed_state
    );
    app.active_thread_id = Some(source_thread_id);
    app.chat_widget.restore_thread_input_state(
        replay_state,
        ThreadInputStateRestoreMode {
            preserve_in_flight_turn: true,
        },
    );
    app.handle_thread_event_replay(diagnostic);
    assert_eq!(pending_lifecycle(&app, &client_id), None);
    assert_eq!(
        pending_lifecycle(&app, &second_client_id),
        Some(PendingSteerLifecycle::AwaitingAcceptance)
    );
    assert!(app.chat_widget.is_agent_turn_running());
    let errors = inserted_history(&mut events);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("offscreen steer rejected"));
    app_server.shutdown().await?;
    server.await??;
    Ok(())
}
