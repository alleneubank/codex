use super::*;
use crate::app::session_lifecycle::ThreadAttachPresentation;
use crate::bottom_pane::MentionBinding;
use crate::chatwidget::PendingSteer;
use crate::chatwidget::PendingSteerLifecycle;
use crate::chatwidget::UserMessage;
use crate::chatwidget::UserMessageHistoryRecord;
use codex_app_server_client::AppServerEvent;
use codex_app_server_protocol::TurnWithdrawPendingInputError;
use codex_app_server_protocol::TurnWithdrawPendingInputErrorReason;
use codex_protocol::user_input::TextElement;
use core_test_support::responses;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::streaming_sse::StreamingSseChunk;
use core_test_support::streaming_sse::StreamingSseServer;
use core_test_support::streaming_sse::start_streaming_sse_server;
use core_test_support::test_codex::TestEnv;
use core_test_support::test_codex::test_env;
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::collections::VecDeque;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::time::timeout;

const MODEL: &str = "gpt-5.4";
const MODEL_PROVIDER_ID: &str = "pending-steer-full-path";
const INITIAL_PROMPT: &str = "Keep this turn open for a pending steer";
const RICH_TEXT: &str = "$skill inspect [Image #1]";
const RICH_HISTORY_TEXT: &str = "[$skill](/tmp/skills/skill/SKILL.md) inspect [Image #1]";
const SIBLING_TEXT: &str = "unrelated pending steer";
const IMAGE_URL: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

struct FullPathFixture {
    app: App,
    events: tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    ops: tokio::sync::mpsc::UnboundedReceiver<Op>,
    app_server: AppServerSession,
    tui: crate::tui::Tui,
    model_server: StreamingSseServer,
    release_first_response: Option<oneshot::Sender<()>>,
    release_second_response: Option<oneshot::Sender<()>>,
    thread_id: ThreadId,
    turn_id: String,
    _codex_home: TempDir,
    _test_env: TestEnv,
}

fn gated_response(response_id: &str) -> (Vec<StreamingSseChunk>, oneshot::Sender<()>) {
    let (release, gate) = oneshot::channel();
    (
        vec![
            StreamingSseChunk {
                gate: None,
                body: responses::sse(vec![ev_response_created(response_id)]),
            },
            StreamingSseChunk {
                gate: Some(gate),
                body: responses::sse(vec![ev_completed(response_id)]),
            },
        ],
        release,
    )
}

fn submit_message(app: &mut App, message: UserMessage) {
    app.chat_widget.restore_user_message_to_composer(message);
    app.chat_widget
        .handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

async fn next_event(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    context: &str,
) -> AppEvent {
    timeout(Duration::from_secs(/*secs*/ 5), events.recv())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {context}"))
        .unwrap_or_else(|| panic!("app event channel closed waiting for {context}"))
}

async fn next_user_turn(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    expected_cwd: &Path,
    expected_thread_id: ThreadId,
    expected_history_text: &str,
) -> AppCommand {
    let mut user_turn = None;
    let mut history_seen = false;
    for _ in 0..8 {
        let context = format!(
            "UserTurn and history entry for {expected_history_text:?} (turn={}, history={history_seen})",
            user_turn.is_some()
        );
        match next_event(events, &context).await {
            AppEvent::CodexOp(turn @ AppCommand::UserTurn { .. }) => {
                assert!(
                    user_turn.replace(turn).is_none(),
                    "received duplicate UserTurn"
                );
            }
            AppEvent::CodexOp(AppCommand::ListSkills { cwds, force_reload }) => {
                assert_eq!(
                    (cwds, force_reload),
                    (vec![expected_cwd.to_path_buf()], true)
                );
            }
            AppEvent::AppendMessageHistoryEntry { thread_id, text } => {
                assert_eq!(
                    (thread_id, text.as_str()),
                    (expected_thread_id, expected_history_text)
                );
                assert!(!history_seen, "received duplicate history entry");
                history_seen = true;
            }
            AppEvent::RefreshPluginMentions | AppEvent::InsertHistoryCell(_) => {}
            event => panic!("expected only UserTurn or startup presentation, got {event:#?}"),
        }
        if history_seen && let Some(user_turn) = user_turn {
            return user_turn;
        }
    }
    panic!("UserTurn and its history entry did not arrive within the bounded event set");
}

fn drain_thread_events(app: &mut App) {
    while let Some(event) = app
        .active_thread_rx
        .as_mut()
        .and_then(|receiver| receiver.try_recv().ok())
    {
        app.handle_thread_event_now(event);
    }
}

async fn next_turn_started(
    app: &mut App,
    app_server: &mut AppServerSession,
    thread_id: ThreadId,
) -> String {
    loop {
        let event = timeout(Duration::from_secs(/*secs*/ 5), app_server.next_event())
            .await
            .expect("app-server should emit turn/started")
            .expect("app-server event stream should remain open");
        let turn_id = if let AppServerEvent::ServerNotification(notification) = &event
            && let ServerNotification::TurnStarted(notification) = notification.as_ref()
            && notification.thread_id == thread_id.to_string()
        {
            Some(notification.turn.id.clone())
        } else {
            None
        };
        app.handle_app_server_event(app_server, event).await;
        drain_thread_events(app);
        if let Some(turn_id) = turn_id {
            return turn_id;
        }
    }
}

async fn wait_for_turn_completed(
    app: &mut App,
    app_server: &mut AppServerSession,
    thread_id: ThreadId,
) {
    loop {
        let event = timeout(Duration::from_secs(/*secs*/ 5), app_server.next_event())
            .await
            .expect("app-server should emit turn/completed")
            .expect("app-server event stream should remain open");
        let completed = matches!(
            &event,
            AppServerEvent::ServerNotification(notification)
                if matches!(
                    notification.as_ref(),
                    ServerNotification::TurnCompleted(notification)
                        if notification.thread_id == thread_id.to_string()
                )
        );
        app.handle_app_server_event(app_server, event).await;
        drain_thread_events(app);
        if completed {
            return;
        }
    }
}

async fn fixture() -> Result<FullPathFixture> {
    let (first_response, release_first_response) = gated_response("response-1");
    let (second_response, release_second_response) = gated_response("response-2");
    let (model_server, _completions) =
        start_streaming_sse_server(vec![first_response, second_response]).await;
    let (mut app, mut events, ops) = make_test_app_with_channels().await;
    let codex_home = tempfile::tempdir()?;
    std::fs::write(
        codex_home.path().join("config.toml"),
        format!(
            r#"
model = "{MODEL}"
model_provider = "{MODEL_PROVIDER_ID}"

[model_providers.{MODEL_PROVIDER_ID}]
name = "Pending steer full path"
base_url = "{}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#,
            model_server.uri()
        ),
    )?;
    app.config.codex_home = codex_home.path().to_path_buf().abs();
    app.config.sqlite = codex_state::SqliteConfig::new_for_testing(codex_home.path().abs());
    app.config.model = Some(MODEL.to_string());
    app.config.model_provider_id = MODEL_PROVIDER_ID.to_string();
    app.config.model_provider = ModelProviderInfo {
        name: "Pending steer full path".to_string(),
        base_url: Some(format!("{}/v1", model_server.uri())),
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        ..ModelProviderInfo::default()
    };
    let test_env = test_env().await.map_err(|error| {
        color_eyre::eyre::eyre!("prepare automatic test environment: {error:#}")
    })?;
    app.config.cwd = test_env.cwd().clone();
    app.config.workspace_roots = vec![test_env.cwd().clone()];

    let state_db =
        crate::init_state_db_for_app_server_target(&app.config, &crate::AppServerTarget::Embedded)
            .await?;
    let environment_manager = Arc::new(
        codex_app_server_client::EnvironmentManager::create_for_tests(
            test_env.exec_server_url().map(str::to_string),
            Some(codex_exec_server::ExecServerRuntimePaths::new(
                std::env::current_exe()?,
                /*codex_linux_sandbox_exe*/ None,
            )?),
        )
        .await,
    );
    let mut app_server = crate::start_app_server_for_picker(
        &app.config,
        &crate::AppServerTarget::Embedded,
        state_db,
        environment_manager,
    )
    .await?;
    let started = app_server.start_thread(&app.config).await?;
    let thread_id = started.session.thread_id;
    let mut tui = crate::tui::test_support::make_test_tui()?;
    app.replace_chat_widget_with_app_server_thread(
        &mut tui,
        started,
        ThreadAttachPresentation::SessionLineage,
        /*initial_user_message*/ None,
    )
    .await?;
    submit_message(&mut app, UserMessage::from(INITIAL_PROMPT));
    let initial_turn = next_user_turn(
        &mut events,
        app.config.cwd.as_path(),
        thread_id,
        INITIAL_PROMPT,
    )
    .await;
    app.submit_thread_op(&mut app_server, thread_id, initial_turn)
        .await?;
    let turn_id = next_turn_started(&mut app, &mut app_server, thread_id).await;
    timeout(
        Duration::from_secs(/*secs*/ 5),
        model_server.wait_for_request_count(/*count*/ 1),
    )
    .await
    .expect("Core should issue the first model request");

    Ok(FullPathFixture {
        app,
        events,
        ops,
        app_server,
        tui,
        model_server,
        release_first_response: Some(release_first_response),
        release_second_response: Some(release_second_response),
        thread_id,
        turn_id,
        _codex_home: codex_home,
        _test_env: test_env,
    })
}

fn rich_message() -> UserMessage {
    UserMessage {
        text: RICH_TEXT.to_string(),
        local_images: Vec::new(),
        remote_image_urls: vec![IMAGE_URL.to_string()],
        text_elements: vec![TextElement::new((0..6).into(), Some("$skill".to_string()))],
        mention_bindings: vec![MentionBinding {
            sigil: '$',
            mention: "skill".to_string(),
            path: "/tmp/skills/skill/SKILL.md".to_string(),
        }],
    }
}

fn expected_rich_inputs(text: &str) -> Vec<UserInput> {
    vec![
        UserInput::Image {
            detail: None,
            url: IMAGE_URL.to_string(),
        },
        UserInput::Text {
            text: text.to_string(),
            text_elements: vec![codex_app_server_protocol::TextElement::new(
                codex_app_server_protocol::ByteRange { start: 0, end: 6 },
                Some("$skill".to_string()),
            )],
        },
    ]
}

fn expected_pending(
    client_id: &str,
    message: UserMessage,
    lifecycle: PendingSteerLifecycle,
) -> PendingSteer {
    PendingSteer::new(
        client_id.to_string(),
        message,
        UserMessageHistoryRecord::UserMessageText,
        lifecycle,
    )
}

async fn submit_pending_steer(
    fixture: &mut FullPathFixture,
    message: UserMessage,
    expected_items: Vec<UserInput>,
    expected_history_text: &str,
) -> Result<String> {
    assert!(fixture.app.chat_widget.is_agent_turn_running());
    submit_message(&mut fixture.app, message.clone());
    let steer = next_user_turn(
        &mut fixture.events,
        fixture.app.config.cwd.as_path(),
        fixture.thread_id,
        expected_history_text,
    )
    .await;
    let (client_id, items) = match &steer {
        AppCommand::UserTurn {
            client_user_message_id,
            items,
            ..
        } => (client_user_message_id.clone(), items.clone()),
        _ => unreachable!(),
    };
    assert_eq!(items, expected_items);
    let expected_awaiting = expected_pending(
        &client_id,
        message.clone(),
        PendingSteerLifecycle::AwaitingAcceptance,
    );
    let state = fixture
        .app
        .chat_widget
        .capture_thread_input_state()
        .expect("thread input state before steer dispatch");
    assert_eq!(state.pending_steers.back(), Some(&expected_awaiting));
    fixture
        .app
        .submit_thread_op(&mut fixture.app_server, fixture.thread_id, steer)
        .await?;
    if let Some(event) = fixture
        .app
        .active_thread_rx
        .as_mut()
        .and_then(|receiver| receiver.try_recv().ok())
    {
        panic!("unexpected thread event after steer submission: {event:#?}");
    }
    let expected_accepted = expected_pending(
        &client_id,
        message,
        PendingSteerLifecycle::Accepted {
            turn_id: fixture.turn_id.clone(),
        },
    );
    let state = fixture
        .app
        .chat_widget
        .capture_thread_input_state()
        .expect("thread input state");
    assert_eq!(state.pending_steers.back(), Some(&expected_accepted));
    Ok(client_id)
}

async fn dispatch_plain_up_withdrawal(
    fixture: &mut FullPathFixture,
    client_id: &str,
) -> Result<AppEvent> {
    fixture
        .app
        .chat_widget
        .handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    let command = match next_event(&mut fixture.events, "pending-steer withdrawal command").await {
        AppEvent::CodexOp(command @ AppCommand::WithdrawPendingSteer { .. }) => command,
        event => panic!("expected only a withdrawal command, got {event:#?}"),
    };
    let AppCommand::WithdrawPendingSteer {
        source_thread_id,
        accepted_turn_id,
        client_user_message_id,
        request_id,
    } = &command
    else {
        unreachable!()
    };
    assert_eq!(
        (
            *source_thread_id,
            accepted_turn_id.as_str(),
            client_user_message_id.as_str()
        ),
        (fixture.thread_id, fixture.turn_id.as_str(), client_id)
    );
    assert!(!request_id.is_empty());
    fixture
        .app
        .handle_event(
            &mut fixture.tui,
            &mut fixture.app_server,
            AppEvent::CodexOp(command),
        )
        .await?;
    match next_event(&mut fixture.events, "pending-steer withdrawal response").await {
        event @ AppEvent::PendingSteerWithdrawalResponse { .. } => Ok(event),
        event => panic!("expected only a withdrawal response, got {event:#?}"),
    }
}

fn request_user_contents(body: &[u8]) -> Result<Vec<Value>> {
    let body: Value = serde_json::from_slice(body)?;
    Ok(body
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|item| item.get("content").cloned())
        .collect())
}

fn expected_core_rich_content(text: &str) -> Value {
    serde_json::json!([
        {"type": "input_image", "image_url": IMAGE_URL, "detail": "high"},
        {"type": "input_text", "text": text},
    ])
}

fn expected_core_text_content(text: &str) -> Value {
    serde_json::json!([{"type": "input_text", "text": text}])
}

fn assert_no_fallback_events(fixture: &mut FullPathFixture) {
    let app_events = std::iter::from_fn(|| fixture.events.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        app_events
            .iter()
            .all(|event| matches!(event, AppEvent::InsertHistoryCell(_))),
        "only accounted presentation events may accompany withdrawal; no Interrupt/UserTurn/start/steer fallback is allowed: {app_events:#?}"
    );
    let core_ops = std::iter::from_fn(|| fixture.ops.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        core_ops.is_empty(),
        "withdrawal must not emit a legacy Core op fallback: {core_ops:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_steer_full_path_withdrawal_wins_and_restores_exact_rich_row() -> Result<()> {
    let mut fixture = fixture().await?;
    let sibling_message = UserMessage::from(SIBLING_TEXT);
    let sibling_id = submit_pending_steer(
        &mut fixture,
        sibling_message.clone(),
        vec![UserInput::Text {
            text: SIBLING_TEXT.to_string(),
            text_elements: Vec::new(),
        }],
        SIBLING_TEXT,
    )
    .await?;
    let message = rich_message();
    let client_id = submit_pending_steer(
        &mut fixture,
        message.clone(),
        expected_rich_inputs(RICH_TEXT),
        RICH_HISTORY_TEXT,
    )
    .await?;
    let expected_sibling = expected_pending(
        &sibling_id,
        sibling_message,
        PendingSteerLifecycle::Accepted {
            turn_id: fixture.turn_id.clone(),
        },
    );
    let expected_target = expected_pending(
        &client_id,
        message.clone(),
        PendingSteerLifecycle::Accepted {
            turn_id: fixture.turn_id.clone(),
        },
    );
    assert_eq!(
        fixture
            .app
            .chat_widget
            .capture_thread_input_state()
            .expect("two accepted rows")
            .pending_steers,
        VecDeque::from([expected_sibling.clone(), expected_target])
    );

    let response = dispatch_plain_up_withdrawal(&mut fixture, &client_id).await?;
    let AppEvent::PendingSteerWithdrawalResponse {
        accepted_turn_id,
        request_id,
        result,
        ..
    } = &response
    else {
        unreachable!()
    };
    assert!(matches!(
        result,
        PendingSteerWithdrawalRequestResult::Withdrawn { turn_id }
            if turn_id == &fixture.turn_id
    ));
    let expected_withdrawn = expected_pending(
        &client_id,
        message,
        PendingSteerLifecycle::WithdrawalInFlight {
            accepted_turn_id: accepted_turn_id.clone(),
            request_id: request_id.clone(),
        },
    );
    assert_eq!(
        fixture
            .app
            .chat_widget
            .capture_thread_input_state()
            .expect("withdrawal in flight")
            .pending_steers,
        VecDeque::from([expected_sibling.clone(), expected_withdrawn.clone()])
    );
    fixture
        .app
        .handle_event(&mut fixture.tui, &mut fixture.app_server, response)
        .await?;
    let withdrawn = match next_event(&mut fixture.events, "typed withdrawn-row handoff").await {
        event @ AppEvent::PendingSteerWithdrawn { .. } => event,
        event => panic!("expected only a withdrawn-row handoff, got {event:#?}"),
    };
    let AppEvent::PendingSteerWithdrawn {
        source_thread_id,
        pending_steer,
    } = &withdrawn
    else {
        unreachable!()
    };
    assert_eq!(
        (*source_thread_id, pending_steer),
        (fixture.thread_id, &expected_withdrawn)
    );
    assert_eq!(
        fixture
            .app
            .chat_widget
            .capture_thread_input_state()
            .expect("target removed after confirmation")
            .pending_steers,
        VecDeque::from([expected_sibling])
    );
    fixture
        .app
        .handle_event(&mut fixture.tui, &mut fixture.app_server, withdrawn)
        .await?;
    assert_eq!(
        fixture.app.chat_widget.composer_text_with_pending(),
        RICH_TEXT
    );
    assert_eq!(fixture.app.chat_widget.composer_cursor(), RICH_TEXT.len());
    fixture
        .app
        .chat_widget
        .handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let resubmitted = next_user_turn(
        &mut fixture.events,
        fixture.app.config.cwd.as_path(),
        fixture.thread_id,
        RICH_HISTORY_TEXT,
    )
    .await;
    let AppCommand::UserTurn {
        client_user_message_id: resubmitted_id,
        items: resubmitted_items,
        ..
    } = resubmitted
    else {
        unreachable!()
    };
    assert_ne!(resubmitted_id, client_id);
    assert_eq!(resubmitted_items, expected_rich_inputs(RICH_TEXT));
    assert_no_fallback_events(&mut fixture);

    fixture
        .release_first_response
        .take()
        .expect("first response gate")
        .send(())
        .expect("first model response should remain gated");
    timeout(
        Duration::from_secs(/*secs*/ 5),
        fixture.model_server.wait_for_request_count(/*count*/ 2),
    )
    .await
    .expect("Core should drain only the unrelated sibling");
    let requests = fixture.model_server.requests().await;
    assert_eq!(requests.len(), 2);
    let contents = request_user_contents(&requests[1])?;
    assert_eq!(
        contents.last(),
        Some(&expected_core_text_content(SIBLING_TEXT))
    );
    assert!(!contents.contains(&expected_core_rich_content(RICH_TEXT)));
    fixture
        .release_second_response
        .take()
        .expect("second response gate")
        .send(())
        .expect("second model response should remain gated");
    wait_for_turn_completed(&mut fixture.app, &mut fixture.app_server, fixture.thread_id).await;
    assert_no_fallback_events(&mut fixture);
    fixture.app_server.shutdown().await?;
    fixture.model_server.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_steer_full_path_drain_wins_and_rejection_does_not_restore() -> Result<()> {
    let mut fixture = fixture().await?;
    let sibling_message = UserMessage::from(SIBLING_TEXT);
    let sibling_id = submit_pending_steer(
        &mut fixture,
        sibling_message.clone(),
        vec![UserInput::Text {
            text: SIBLING_TEXT.to_string(),
            text_elements: Vec::new(),
        }],
        SIBLING_TEXT,
    )
    .await?;
    let message = rich_message();
    let client_id = submit_pending_steer(
        &mut fixture,
        message.clone(),
        expected_rich_inputs(RICH_TEXT),
        RICH_HISTORY_TEXT,
    )
    .await?;
    let expected_queue = VecDeque::from([
        expected_pending(
            &sibling_id,
            sibling_message,
            PendingSteerLifecycle::Accepted {
                turn_id: fixture.turn_id.clone(),
            },
        ),
        expected_pending(
            &client_id,
            message,
            PendingSteerLifecycle::Accepted {
                turn_id: fixture.turn_id.clone(),
            },
        ),
    ]);
    assert_eq!(
        fixture
            .app
            .chat_widget
            .capture_thread_input_state()
            .expect("two accepted rows")
            .pending_steers,
        expected_queue
    );

    fixture
        .release_first_response
        .take()
        .expect("first response gate")
        .send(())
        .expect("first model response should remain gated");
    timeout(
        Duration::from_secs(/*secs*/ 5),
        fixture.model_server.wait_for_request_count(/*count*/ 2),
    )
    .await
    .expect("Core should drain the steer into the second model request");
    let requests = fixture.model_server.requests().await;
    assert_eq!(requests.len(), 2);
    let contents = request_user_contents(&requests[1])?;
    assert_eq!(
        &contents[contents.len() - 2..],
        &[
            expected_core_text_content(SIBLING_TEXT),
            expected_core_rich_content(RICH_TEXT),
        ]
    );

    let response = dispatch_plain_up_withdrawal(&mut fixture, &client_id).await?;
    let AppEvent::PendingSteerWithdrawalResponse { result, .. } = &response else {
        unreachable!()
    };
    assert!(matches!(
        result,
        PendingSteerWithdrawalRequestResult::Rejected(rejection)
            if rejection == &crate::app_event::PendingSteerWithdrawalServerRejection {
                code: -32600,
                message: "client user message id is not pending".to_string(),
                data: Some(TurnWithdrawPendingInputError {
                    reason: TurnWithdrawPendingInputErrorReason::NotPending,
                    expected_turn_id: Some(fixture.turn_id.clone()),
                    actual_turn_id: Some(fixture.turn_id.clone()),
                }),
            }
    ));
    fixture
        .app
        .handle_event(&mut fixture.tui, &mut fixture.app_server, response)
        .await?;
    let state = fixture
        .app
        .chat_widget
        .capture_thread_input_state()
        .expect("thread input state after rejection");
    assert_eq!(state.pending_steers, expected_queue);
    fixture
        .app
        .chat_widget
        .handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_no_fallback_events(&mut fixture);

    fixture
        .release_second_response
        .take()
        .expect("second response gate")
        .send(())
        .expect("second model response should remain gated");
    wait_for_turn_completed(&mut fixture.app, &mut fixture.app_server, fixture.thread_id).await;
    assert_no_fallback_events(&mut fixture);
    fixture.app_server.shutdown().await?;
    fixture.model_server.shutdown().await;
    Ok(())
}
