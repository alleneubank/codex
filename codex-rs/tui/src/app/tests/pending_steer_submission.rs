use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;

use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCResponse;
use codex_protocol::user_input::TextElement;
use futures::SinkExt;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use super::*;
use crate::bottom_pane::LocalImageAttachment;
use crate::bottom_pane::MentionBinding;
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
    expected_params: Option<serde_json::Value>,
    reply: Reply,
}

impl Step {
    fn result(method: &'static str, result: serde_json::Value) -> Self {
        Self {
            method,
            expected_params: None,
            reply: Reply::Result(result),
        }
    }

    fn error(method: &'static str, message: &str, data: Option<serde_json::Value>) -> Self {
        Self {
            method,
            expected_params: None,
            reply: Reply::Error(JSONRPCErrorError {
                code: -32602,
                message: message.to_string(),
                data,
            }),
        }
    }

    fn exact_result(
        method: &'static str,
        expected_params: serde_json::Value,
        result: serde_json::Value,
    ) -> Self {
        Self {
            method,
            expected_params: Some(expected_params),
            reply: Reply::Result(result),
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
                    if let Some(expected_params) = step.expected_params {
                        assert_eq!(request.params, Some(expected_params));
                    }
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
    pending_submission_fixture_with_messages(
        UserMessage::from("pending steer"),
        UserMessage::from("second pending steer"),
    )
    .await
}

async fn pending_submission_fixture_with_messages(
    first: UserMessage,
    second: UserMessage,
) -> Result<PendingFixture> {
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
    app.chat_widget.restore_user_message_to_composer(first);
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
    app.chat_widget.restore_user_message_to_composer(second);
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
                expected_params: None,
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

async fn begin_withdrawal(
    app: &mut App,
    events: &mut AppEvents,
    thread_id: ThreadId,
    client_id: &str,
) -> AppCommand {
    assert_eq!(app.active_thread_id, Some(thread_id));
    assert_eq!(app.chat_widget.thread_id(), Some(thread_id));
    let request_id = Uuid::new_v4().to_string();
    let accepted_turn_id = app
        .chat_widget
        .begin_pending_steer_withdrawal(client_id, &request_id)
        .expect("accepted pending steer");
    app.app_event_tx
        .send(AppEvent::CodexOp(AppCommand::WithdrawPendingSteer {
            source_thread_id: thread_id,
            accepted_turn_id,
            client_user_message_id: client_id.to_string(),
            request_id,
        }));
    loop {
        if let Some(AppEvent::CodexOp(op @ AppCommand::WithdrawPendingSteer { .. })) =
            events.recv().await
        {
            return op;
        }
    }
}

async fn apply_withdrawal_response(
    app: &mut App,
    events: &mut AppEvents,
) -> Result<crate::chatwidget::PendingSteerWithdrawalEffect> {
    loop {
        if let Some(AppEvent::PendingSteerWithdrawalResponse {
            source_thread_id,
            accepted_turn_id,
            client_user_message_id,
            request_id,
            result,
        }) = events.recv().await
        {
            return app
                .reconcile_pending_steer_withdrawal_response(
                    source_thread_id,
                    &accepted_turn_id,
                    &client_user_message_id,
                    &request_id,
                    result,
                )
                .await;
        }
    }
}

#[tokio::test]
async fn pending_steer_withdrawal_routes_success_rejection_and_uncertainty() -> Result<()> {
    for (reply, expected_lifecycle) in [
        (
            Reply::Result(serde_json::json!({"turnId": "turn-initial"})),
            None,
        ),
        (
            Reply::Error(JSONRPCErrorError {
                code: -32600,
                message: "not pending".to_string(),
                data: None,
            }),
            Some(PendingSteerLifecycle::Accepted {
                turn_id: "turn-initial".to_string(),
            }),
        ),
        (
            Reply::Result(serde_json::json!({"wrong": true})),
            Some(PendingSteerLifecycle::WithdrawalUncertain {
                accepted_turn_id: "turn-initial".to_string(),
                request_id: String::new(),
            }),
        ),
        (
            Reply::Disconnect,
            Some(PendingSteerLifecycle::WithdrawalUncertain {
                accepted_turn_id: "turn-initial".to_string(),
                request_id: String::new(),
            }),
        ),
    ] {
        let (mut app, mut events, _op, thread_id, client_id, _) =
            pending_submission_fixture().await?;
        app.chat_widget.reconcile_pending_steer_submission(
            &client_id,
            crate::chatwidget::PendingSteerSubmissionOutcome::SteerAccepted {
                turn_id: "turn-initial".to_string(),
            },
        );
        let command = begin_withdrawal(&mut app, &mut events, thread_id, &client_id).await;
        assert_eq!(
            app.chat_widget
                .begin_pending_steer_withdrawal(&client_id, "second-request"),
            None
        );
        let request_id = match &command {
            AppCommand::WithdrawPendingSteer {
                source_thread_id,
                accepted_turn_id,
                client_user_message_id,
                request_id,
            } => {
                assert_eq!(
                    (
                        *source_thread_id,
                        accepted_turn_id.as_str(),
                        client_user_message_id.as_str()
                    ),
                    (thread_id, "turn-initial", client_id.as_str())
                );
                request_id.clone()
            }
            _ => unreachable!(),
        };
        let (mut server_session, requests, server) = start_scripted_app_server(
            &app.config,
            vec![Step {
                method: "turn/withdrawPendingInput",
                expected_params: None,
                reply,
            }],
        )
        .await?;
        assert!(
            app.try_submit_active_thread_op_via_app_server(
                &mut server_session,
                thread_id,
                &command
            )
            .await?
        );
        let effect = apply_withdrawal_response(&mut app, &mut events).await?;
        let was_rejected = effect == crate::chatwidget::PendingSteerWithdrawalEffect::Rejected;
        if expected_lifecycle.is_none() {
            assert!(matches!(
                effect,
                crate::chatwidget::PendingSteerWithdrawalEffect::Withdrawn(_)
            ));
        } else {
            assert!(matches!(
                app.active_thread_rx.as_mut().unwrap().try_recv(),
                Ok(ThreadBufferedEvent::Notification(_))
            ));
            assert!(app.active_thread_rx.as_mut().unwrap().try_recv().is_err());
        }
        let actual = pending_lifecycle(&app, &client_id).map(|lifecycle| match lifecycle {
            PendingSteerLifecycle::WithdrawalUncertain {
                accepted_turn_id, ..
            } => PendingSteerLifecycle::WithdrawalUncertain {
                accepted_turn_id,
                request_id: String::new(),
            },
            lifecycle => lifecycle,
        });
        assert_eq!(actual, expected_lifecycle);
        assert_eq!(
            requests.lock().unwrap().as_slice(),
            std::slice::from_ref(&client_id)
        );
        assert!(!request_id.is_empty());
        if was_rejected {
            let retry = begin_withdrawal(&mut app, &mut events, thread_id, &client_id).await;
            let AppCommand::WithdrawPendingSteer {
                request_id: retry_id,
                ..
            } = retry
            else {
                unreachable!()
            };
            assert_ne!(request_id, retry_id);
        }
        let _ = server_session.shutdown().await;
        server.await??;
    }
    Ok(())
}

#[tokio::test]
async fn withdrawal_survives_completion_but_interrupt_invalidates_it() -> Result<()> {
    for (status, expected_present) in [
        (TurnStatus::Completed, true),
        (TurnStatus::Interrupted, false),
    ] {
        let (mut app, mut events, _op, thread_id, client_id, _) =
            pending_submission_fixture().await?;
        app.chat_widget.reconcile_pending_steer_submission(
            &client_id,
            crate::chatwidget::PendingSteerSubmissionOutcome::SteerAccepted {
                turn_id: "turn-initial".to_string(),
            },
        );
        let _command = begin_withdrawal(&mut app, &mut events, thread_id, &client_id).await;
        app.handle_thread_event_now(ThreadBufferedEvent::Notification(Box::new(
            turn_completed_notification(thread_id, "turn-initial", status),
        )));
        assert_eq!(
            pending_lifecycle(&app, &client_id).is_some(),
            expected_present
        );
    }
    Ok(())
}

#[tokio::test]
async fn offscreen_withdrawal_result_updates_only_source_state() -> Result<()> {
    let (mut app, mut events, _op, thread_id, client_id, _) = pending_submission_fixture().await?;
    app.chat_widget.reconcile_pending_steer_submission(
        &client_id,
        crate::chatwidget::PendingSteerSubmissionOutcome::SteerAccepted {
            turn_id: "turn-initial".to_string(),
        },
    );
    let command = begin_withdrawal(&mut app, &mut events, thread_id, &client_id).await;
    let displayed_state = app.chat_widget.capture_thread_input_state();
    let source_store = Arc::clone(&app.thread_event_channels[&thread_id].store);
    {
        let mut store = source_store.lock().await;
        store.active = false;
        store.input_state = displayed_state.clone();
    }
    app.active_thread_id = Some(ThreadId::new());
    let AppCommand::WithdrawPendingSteer {
        accepted_turn_id,
        request_id,
        ..
    } = command
    else {
        unreachable!()
    };
    assert_eq!(
        app.reconcile_pending_steer_withdrawal_response(
            thread_id,
            &accepted_turn_id,
            &client_id,
            &request_id,
            PendingSteerWithdrawalRequestResult::Rejected(
                crate::app_event::PendingSteerWithdrawalServerRejection {
                    code: -32600,
                    message: "not pending".to_string(),
                    data: None,
                },
            ),
        )
        .await?,
        crate::chatwidget::PendingSteerWithdrawalEffect::Rejected
    );
    assert_eq!(
        app.chat_widget.capture_thread_input_state(),
        displayed_state
    );
    let store = source_store.lock().await;
    assert!(matches!(
        store.input_state.as_ref().unwrap().pending_steers[0].lifecycle,
        PendingSteerLifecycle::Accepted { .. }
    ));
    assert!(matches!(
        store.buffer.back(),
        Some(ThreadBufferedEvent::Notification(_))
    ));
    Ok(())
}

#[tokio::test]
async fn app_event_withdrawal_handoff_stays_bound_to_rich_source_row() -> Result<()> {
    let rich_message = UserMessage {
        text: "$skill inspect [Image #1]".to_string(),
        local_images: vec![LocalImageAttachment {
            placeholder: "[Image #1]".to_string(),
            path: PathBuf::from("/tmp/pending-local.png"),
        }],
        remote_image_urls: vec!["https://example.test/pending.png".to_string()],
        text_elements: vec![TextElement::new((0..6).into(), Some("$skill".to_string()))],
        mention_bindings: vec![MentionBinding {
            sigil: '$',
            mention: "skill".to_string(),
            path: "/tmp/skills/skill/SKILL.md".to_string(),
        }],
    };
    let (mut app, mut events, _op, source_thread_id, client_id, _) =
        pending_submission_fixture_with_messages(
            rich_message,
            UserMessage::from("unrelated pending steer"),
        )
        .await?;
    app.chat_widget.reconcile_pending_steer_submission(
        &client_id,
        crate::chatwidget::PendingSteerSubmissionOutcome::SteerAccepted {
            turn_id: "turn-initial".to_string(),
        },
    );
    let command = begin_withdrawal(&mut app, &mut events, source_thread_id, &client_id).await;
    let (accepted_turn_id, request_id) = match &command {
        AppCommand::WithdrawPendingSteer {
            accepted_turn_id,
            request_id,
            ..
        } => (accepted_turn_id.clone(), request_id.clone()),
        _ => unreachable!(),
    };

    app.store_active_thread_receiver().await;
    app.clear_active_thread().await;
    let displayed_thread_id = ThreadId::new();
    app.enqueue_primary_thread_session(
        test_thread_session(displayed_thread_id, app.config.cwd.to_path_buf()),
        Vec::new(),
    )
    .await?;
    let displayed_state = app.chat_widget.capture_thread_input_state();
    let source_store = Arc::clone(&app.thread_event_channels[&source_thread_id].store);
    let source_state = source_store
        .lock()
        .await
        .input_state
        .clone()
        .expect("source input state was stored during the thread switch");
    let expected_row = source_state
        .pending_steers
        .iter()
        .find(|pending| pending.client_user_message_id == client_id)
        .cloned()
        .expect("rich target row remains pending before the response");
    let mut expected_source_state = source_state.clone();
    expected_source_state
        .pending_steers
        .retain(|pending| pending.client_user_message_id != client_id);
    let (mut expected_chat, _expected_tx, _expected_events, _expected_ops) =
        make_chatwidget_manual_with_sender().await;
    expected_chat.restore_thread_input_state(
        Some(expected_source_state),
        ThreadInputStateRestoreMode {
            preserve_in_flight_turn: true,
        },
    );
    expected_chat.restore_withdrawn_pending_steer(expected_row.clone());
    let expected_source_state = expected_chat
        .capture_thread_input_state()
        .expect("expected restored source input state");

    let (mut app_server, _requests, server) = start_scripted_app_server(
        &app.config,
        vec![Step::exact_result(
            "turn/withdrawPendingInput",
            serde_json::json!({
                "threadId": source_thread_id.to_string(),
                "expectedTurnId": accepted_turn_id.clone(),
                "clientUserMessageId": client_id.clone(),
            }),
            serde_json::json!({"turnId": "turn-initial"}),
        )],
    )
    .await?;
    let mut tui = crate::tui::test_support::make_test_tui()?;
    app.handle_event(
        &mut tui,
        &mut app_server,
        AppEvent::CodexOp(command.clone()),
    )
    .await?;
    let response = loop {
        if let Some(event @ AppEvent::PendingSteerWithdrawalResponse { .. }) = events.recv().await {
            break event;
        }
    };

    for (stale_request_id, response_turn_id) in [
        ("stale-request", "turn-initial"),
        (request_id.as_str(), "turn-mismatch"),
    ] {
        app.handle_event(
            &mut tui,
            &mut app_server,
            AppEvent::PendingSteerWithdrawalResponse {
                source_thread_id,
                accepted_turn_id: "turn-initial".to_string(),
                client_user_message_id: client_id.clone(),
                request_id: stale_request_id.to_string(),
                result: PendingSteerWithdrawalRequestResult::Withdrawn {
                    turn_id: response_turn_id.to_string(),
                },
            },
        )
        .await?;
    }
    assert!(
        !std::iter::from_fn(|| events.try_recv().ok())
            .any(|event| matches!(event, AppEvent::PendingSteerWithdrawn { .. }))
    );
    assert_eq!(source_store.lock().await.input_state, Some(source_state));
    assert_eq!(
        app.chat_widget.capture_thread_input_state(),
        displayed_state
    );

    app.handle_event(&mut tui, &mut app_server, response)
        .await?;
    let handoff = loop {
        if let Some(event @ AppEvent::PendingSteerWithdrawn { .. }) = events.recv().await {
            break event;
        }
    };
    let AppEvent::PendingSteerWithdrawn {
        source_thread_id: handoff_thread_id,
        pending_steer: withdrawn,
    } = &handoff
    else {
        unreachable!()
    };
    assert_eq!(
        (*handoff_thread_id, withdrawn),
        (source_thread_id, &expected_row)
    );
    app.handle_event(&mut tui, &mut app_server, handoff).await?;
    assert_eq!(
        source_store.lock().await.input_state,
        Some(expected_source_state.clone())
    );
    assert_eq!(
        app.chat_widget.capture_thread_input_state(),
        displayed_state
    );
    app.active_thread_id = Some(source_thread_id);
    app.chat_widget.restore_thread_input_state(
        Some(expected_source_state.clone()),
        ThreadInputStateRestoreMode {
            preserve_in_flight_turn: true,
        },
    );
    assert_eq!(
        app.chat_widget.capture_thread_input_state(),
        Some(expected_source_state)
    );
    app_server.shutdown().await?;
    server.await??;
    Ok(())
}
