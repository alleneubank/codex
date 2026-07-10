use super::*;
use codex_app_server_protocol::ThreadGoal as AppThreadGoal;
use codex_app_server_protocol::ThreadGoalStatus as AppThreadGoalStatus;
use pretty_assertions::assert_eq;
use std::time::Instant;

fn press_stash(chat: &mut ChatWidget) {
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
}

fn set_composer_text(chat: &mut ChatWidget, text: &str) {
    chat.bottom_pane
        .set_composer_text(text.to_string(), Vec::new(), Vec::new());
}

#[tokio::test]
async fn ctrl_s_stashes_and_restores_prompt() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_composer_text(&mut chat, "long draft");

    press_stash(&mut chat);
    assert_eq!(chat.bottom_pane.composer_text(), "");

    press_stash(&mut chat);
    assert_eq!(chat.bottom_pane.composer_text(), "long draft");
}

#[tokio::test]
async fn prompt_stash_round_trip_preserves_rich_draft_and_cursor() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let mention = "$skill";
    let image = "[Image #1]";
    let paste_placeholder = "[Pasted Content 5001 chars]";
    let text = format!("lead {mention} {image} {paste_placeholder} tail");
    let element = |placeholder: &str| {
        let start = text.find(placeholder).expect("placeholder in draft");
        TextElement::new(
            (start..start + placeholder.len()).into(),
            Some(placeholder.to_string()),
        )
    };
    let text_elements = vec![element(mention), element(image), element(paste_placeholder)];
    let local_image = PathBuf::from("/tmp/stashed-local.png");
    let remote_image = "https://example.com/stashed-remote.png".to_string();
    let pending_pastes = vec![(paste_placeholder.to_string(), "x".repeat(5_001))];
    let mention_bindings = vec![MentionBinding {
        sigil: '$',
        mention: "skill".to_string(),
        path: "skill:///tmp/SKILL.md".to_string(),
    }];

    chat.set_remote_image_urls(vec![remote_image]);
    chat.bottom_pane.set_composer_text_with_mention_bindings(
        text,
        text_elements,
        vec![local_image],
        mention_bindings,
    );
    chat.bottom_pane.set_composer_pending_pastes(pending_pastes);
    chat.bottom_pane.set_composer_cursor(/*cursor*/ 3);
    let expected = chat.bottom_pane.composer_draft_snapshot();

    press_stash(&mut chat);
    assert!(chat.bottom_pane.composer_is_empty());

    press_stash(&mut chat);
    assert_eq!(chat.bottom_pane.composer_draft_snapshot(), expected);
}

#[tokio::test]
async fn prompt_stash_never_overwrites_a_new_draft() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_composer_text(&mut chat, "original draft");
    press_stash(&mut chat);
    set_composer_text(&mut chat, "new draft");

    press_stash(&mut chat);
    assert_eq!(chat.bottom_pane.composer_text(), "new draft");

    set_composer_text(&mut chat, "");
    press_stash(&mut chat);
    assert_eq!(chat.bottom_pane.composer_text(), "original draft");
}

#[tokio::test]
async fn empty_stash_keypress_is_a_no_op() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    press_stash(&mut chat);

    assert!(chat.bottom_pane.composer_is_empty());
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn accepted_model_turn_restores_stash_on_live_idle_completion() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    set_composer_text(&mut chat, "original draft");
    press_stash(&mut chat);
    set_composer_text(&mut chat, "short question");

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let _ = next_submit_op(&mut op_rx);
    handle_turn_started(&mut chat, "turn-1");
    handle_turn_completed(&mut chat, "turn-1", /*duration_ms*/ None);

    assert_eq!(chat.bottom_pane.composer_text(), "original draft");
}

#[tokio::test]
async fn unrelated_running_turn_does_not_restore_new_stash() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.on_task_started();
    set_composer_text(&mut chat, "original draft");
    press_stash(&mut chat);

    chat.on_task_complete(
        /*last_agent_message*/ None, /*duration_ms*/ None, /*from_replay*/ false,
    );
    assert!(chat.bottom_pane.composer_is_empty());

    press_stash(&mut chat);
    assert_eq!(chat.bottom_pane.composer_text(), "original draft");
}

#[tokio::test]
async fn replayed_completion_does_not_restore_armed_stash() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    set_composer_text(&mut chat, "original draft");
    press_stash(&mut chat);
    chat.submit_user_message(UserMessage::from("short question"));
    let _ = next_submit_op(&mut op_rx);

    chat.on_task_complete(
        /*last_agent_message*/ None, /*duration_ms*/ None, /*from_replay*/ true,
    );
    assert!(chat.bottom_pane.composer_is_empty());

    chat.on_task_complete(
        /*last_agent_message*/ None, /*duration_ms*/ None, /*from_replay*/ false,
    );
    assert_eq!(chat.bottom_pane.composer_text(), "original draft");
}

#[tokio::test]
async fn queued_follow_up_defers_automatic_stash_restore() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    set_composer_text(&mut chat, "original draft");
    press_stash(&mut chat);
    chat.submit_user_message(UserMessage::from("short question"));
    let _ = next_submit_op(&mut op_rx);
    handle_turn_started(&mut chat, "turn-1");
    chat.queue_user_message(UserMessage::from("queued follow-up"));

    handle_turn_completed(&mut chat, "turn-1", /*duration_ms*/ None);
    let _ = next_submit_op(&mut op_rx);
    assert!(chat.bottom_pane.composer_is_empty());

    handle_turn_started(&mut chat, "turn-2");
    handle_turn_completed(&mut chat, "turn-2", /*duration_ms*/ None);
    assert_eq!(chat.bottom_pane.composer_text(), "original draft");
}

#[tokio::test]
async fn active_goal_defers_automatic_stash_restore() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    set_composer_text(&mut chat, "original draft");
    press_stash(&mut chat);
    chat.submit_user_message(UserMessage::from("short question"));
    let _ = next_submit_op(&mut op_rx);
    chat.current_goal_status = Some(GoalStatusState::new(
        AppThreadGoal {
            thread_id: thread_id.to_string(),
            objective: "continue autonomously".to_string(),
            status: AppThreadGoalStatus::Active,
            token_budget: None,
            tokens_used: 0,
            time_used_seconds: 0,
            created_at: 1,
            updated_at: 1,
        },
        Instant::now(),
    ));

    chat.on_task_complete(
        /*last_agent_message*/ None, /*duration_ms*/ None, /*from_replay*/ false,
    );
    assert!(chat.bottom_pane.composer_is_empty());

    chat.current_goal_status = None;
    chat.on_task_complete(
        /*last_agent_message*/ None, /*duration_ms*/ None, /*from_replay*/ false,
    );
    assert_eq!(chat.bottom_pane.composer_text(), "original draft");
}

#[tokio::test]
async fn local_shell_submission_does_not_arm_prompt_stash() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    set_composer_text(&mut chat, "original draft");
    press_stash(&mut chat);

    chat.submit_user_message(UserMessage::from("!pwd"));
    chat.on_task_complete(
        /*last_agent_message*/ None, /*duration_ms*/ None, /*from_replay*/ false,
    );
    assert!(chat.bottom_pane.composer_is_empty());

    press_stash(&mut chat);
    assert_eq!(chat.bottom_pane.composer_text(), "original draft");
}

#[tokio::test]
async fn rejected_model_submission_does_not_arm_prompt_stash() {
    let (mut chat, _rx, op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    set_composer_text(&mut chat, "original draft");
    press_stash(&mut chat);
    drop(op_rx);

    chat.submit_user_message(UserMessage::from("rejected question"));
    chat.on_task_complete(
        /*last_agent_message*/ None, /*duration_ms*/ None, /*from_replay*/ false,
    );
    assert!(chat.bottom_pane.composer_is_empty());

    press_stash(&mut chat);
    assert_eq!(chat.bottom_pane.composer_text(), "original draft");
}

#[tokio::test]
async fn history_search_keeps_ctrl_s_forward_navigation() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_composer_text(&mut chat, "search draft");

    chat.handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert!(chat.bottom_pane.composer_history_search_active());
    press_stash(&mut chat);
    assert!(chat.bottom_pane.composer_history_search_active());
    chat.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    set_composer_text(&mut chat, "");
    press_stash(&mut chat);
    assert!(chat.bottom_pane.composer_is_empty());
}

#[tokio::test]
async fn prompt_stash_follows_thread_input_state_and_none_clears_it() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_composer_text(&mut chat, "thread draft");
    press_stash(&mut chat);
    let input_state = chat
        .capture_thread_input_state()
        .expect("thread input state");

    chat.restore_thread_input_state(/*input_state*/ None);
    press_stash(&mut chat);
    assert!(chat.bottom_pane.composer_is_empty());

    chat.restore_thread_input_state(Some(input_state));
    press_stash(&mut chat);
    assert_eq!(chat.bottom_pane.composer_text(), "thread draft");
}
