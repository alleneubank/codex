use super::*;
use crate::bottom_pane::HistoryEntry;
use crate::key_hint;
use codex_app_server_protocol::ThreadGoalStatus;
use pretty_assertions::assert_eq;
use std::time::Instant;

const DRAFT: &str = "original draft";

fn stash_key() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)
}

fn plain_key(ch: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
}

fn set_text(chat: &mut ChatWidget, text: &str) {
    chat.apply_external_edit(text.to_string());
}

fn stash(chat: &mut ChatWidget) {
    set_text(chat, DRAFT);
    chat.handle_key_event(stash_key());
    assert!(chat.bottom_pane.composer_is_empty());
}

fn restore(chat: &mut ChatWidget, context: &str) {
    chat.handle_key_event(stash_key());
    assert_eq!(chat.bottom_pane.composer_text(), DRAFT, "{context}");
}

async fn armed_stash() -> (ChatWidget, tokio::sync::mpsc::UnboundedReceiver<Op>) {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    stash(&mut chat);
    chat.submit_user_message(UserMessage::from("short question"));
    let _ = next_submit_op(&mut op_rx);
    handle_turn_started(&mut chat, "turn-1");
    (chat, op_rx)
}

fn complete_task(chat: &mut ChatWidget, from_replay: bool) {
    chat.on_task_complete(
        /*last_agent_message*/ None,
        /*duration_ms*/ None,
        from_replay,
    );
}

#[tokio::test]
async fn manual_stash_is_lossless_non_overwriting_and_empty_stash_is_a_no_op() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    stash(&mut chat);
    set_text(&mut chat, "new draft");
    chat.handle_key_event(stash_key());
    assert_eq!(chat.bottom_pane.composer_text(), "new draft");
    set_text(&mut chat, "");
    restore(&mut chat, "manual restore");
    set_text(&mut chat, "");
    chat.handle_key_event(stash_key());
    assert!(chat.bottom_pane.composer_is_empty());
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let text = "$s [i] [p]";
    chat.set_remote_image_urls(vec!["https://example.test/i".to_string()]);
    chat.bottom_pane.set_composer_text_with_mention_bindings(
        text.to_string(),
        vec![TextElement::new((0..2).into(), Some("$s".to_string()))],
        vec![PathBuf::from("/tmp/i")],
        vec![MentionBinding {
            sigil: '$',
            mention: "s".to_string(),
            path: "skill:///s".to_string(),
        }],
    );
    chat.bottom_pane
        .set_composer_pending_pastes(vec![("[p]".to_string(), "payload".to_string())]);
    chat.bottom_pane.set_composer_cursor(/*cursor*/ 3);
    let expected = chat.bottom_pane.composer_draft_snapshot();
    chat.handle_key_event(stash_key());
    chat.handle_key_event(stash_key());
    let actual = chat.bottom_pane.composer_draft_snapshot();
    assert_eq!(actual, expected, "rich draft");
}

#[tokio::test]
async fn stash_restoration_is_bound_to_an_accepted_live_turn_and_idle_state() {
    let (mut idle, _op_rx) = armed_stash().await;
    handle_turn_completed(&mut idle, "turn-0", /*duration_ms*/ None);
    assert!(idle.bottom_pane.composer_is_empty(), "unrelated turn");
    handle_turn_completed(&mut idle, "turn-1", /*duration_ms*/ None);
    assert_eq!(idle.bottom_pane.composer_text(), DRAFT);
    let (mut busy, _op_rx) = armed_stash().await;
    set_text(&mut busy, "new draft");
    handle_turn_completed(&mut busy, "turn-1", /*duration_ms*/ None);
    assert_eq!(busy.bottom_pane.composer_text(), "new draft");
    set_text(&mut busy, "");
    restore(&mut busy, "non-overwrite");
    let (mut queued, mut op_rx) = armed_stash().await;
    queued.queue_user_message(UserMessage::from("queued follow-up"));
    handle_turn_completed(&mut queued, "turn-1", /*duration_ms*/ None);
    let _ = next_submit_op(&mut op_rx);
    assert!(queued.bottom_pane.composer_is_empty(), "queued follow-up");
    handle_turn_started(&mut queued, "turn-2");
    handle_turn_completed(&mut queued, "turn-2", /*duration_ms*/ None);
    assert_eq!(queued.bottom_pane.composer_text(), DRAFT);
    let (mut goal, _op_rx) = armed_stash().await;
    goal.current_goal_status = Some(GoalStatusState::new(
        status_and_layout::test_thread_goal(ThreadGoalStatus::Active, None, 0),
        Instant::now(),
    ));
    complete_task(&mut goal, /*from_replay*/ false);
    assert!(goal.bottom_pane.composer_is_empty(), "active goal");
    goal.current_goal_status = None;
    complete_task(&mut goal, /*from_replay*/ false);
    assert_eq!(goal.bottom_pane.composer_text(), DRAFT);
    let (mut replayed, _op_rx) = armed_stash().await;
    replayed.turn_lifecycle.last_turn_id = Some("turn-0".to_string());
    complete_task(&mut replayed, /*from_replay*/ true);
    restore(&mut replayed, "replayed completion");
    for (context, message, reject) in [
        ("local shell", "!pwd", false),
        ("rejected model turn", "rejected question", true),
    ] {
        let (mut chat, _rx, op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.thread_id = Some(ThreadId::new());
        stash(&mut chat);
        if reject {
            drop(op_rx);
        }
        chat.submit_user_message(UserMessage::from(message));
        complete_task(&mut chat, /*from_replay*/ false);
        assert!(chat.bottom_pane.composer_is_empty(), "{context}");
        restore(&mut chat, context);
    }
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    for entry in ["search older", "search newer"] {
        chat.bottom_pane
            .record_replayed_user_message_history(HistoryEntry::new(entry.to_string()));
    }
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    for ch in "search".chars() {
        chat.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert_eq!(chat.bottom_pane.composer_text(), "search older");
    chat.handle_key_event(stash_key());
    assert!(chat.bottom_pane.composer_history_search_active());
    assert_eq!(chat.bottom_pane.composer_text(), "search newer");
    for (event, remap) in [
        (stash_key(), None),
        (plain_key('\u{0013}'), None),
        (plain_key('z'), Some('z')),
    ] {
        let (mut chat, _sender, _rx, _op_rx) = make_chatwidget_manual_with_sender().await;
        if let Some(key) = remap {
            chat.chat_keymap.stash_prompt = vec![key_hint::plain(KeyCode::Char(key))];
        }
        set_text(&mut chat, "visible prefix ");
        for ch in "buffered suffix".chars() {
            chat.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert!(chat.bottom_pane.is_in_paste_burst());
        chat.handle_key_event(event);
        let expected = "visible prefix buffered suffix";
        let stash = chat.prompt_stash.as_ref().expect("stashed prompt");
        assert_eq!(stash.composer.text, expected);
        chat.handle_key_event(event);
        assert_eq!(chat.bottom_pane.composer_text(), expected);
    }
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_text(&mut chat, "thread draft");
    chat.handle_key_event(stash_key());
    let input = chat
        .capture_thread_input_state()
        .expect("thread input state");
    chat.restore_thread_input_state(/*input_state*/ None);
    chat.handle_key_event(stash_key());
    assert!(chat.bottom_pane.composer_is_empty());
    chat.restore_thread_input_state(Some(input));
    chat.handle_key_event(stash_key());
    assert_eq!(chat.bottom_pane.composer_text(), "thread draft");
}

#[tokio::test]
async fn live_steers_keep_stash_bound_to_the_running_turn() {
    let (mut bound, mut op_rx) = armed_stash().await;
    set_text(&mut bound, "steer after stash was bound");
    bound.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let _ = next_submit_op(&mut op_rx);
    handle_turn_completed(&mut bound, "turn-1", /*duration_ms*/ None);
    assert_eq!(bound.bottom_pane.composer_text(), DRAFT);

    let (mut created_during_turn, _rx, mut op_rx) =
        make_chatwidget_manual(/*model_override*/ None).await;
    created_during_turn.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut created_during_turn, "turn-1");
    stash(&mut created_during_turn);
    set_text(&mut created_during_turn, "steer after stash was created");
    created_during_turn.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let _ = next_submit_op(&mut op_rx);
    handle_turn_completed(
        &mut created_during_turn,
        "turn-1",
        /*duration_ms*/ None,
    );
    assert_eq!(created_during_turn.bottom_pane.composer_text(), DRAFT);
}

#[tokio::test]
async fn buffered_input_prevents_automatic_stash_restoration() {
    let (mut chat, _op_rx) = armed_stash().await;
    for ch in "new draft".chars() {
        chat.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    assert!(chat.bottom_pane.is_in_paste_burst());

    handle_turn_completed(&mut chat, "turn-1", /*duration_ms*/ None);
    assert!(
        chat.prompt_stash.is_some(),
        "stash remains available manually"
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(chat.bottom_pane.composer_text(), "new draft");
    set_text(&mut chat, "");
    restore(&mut chat, "buffered draft non-overwrite");
}
