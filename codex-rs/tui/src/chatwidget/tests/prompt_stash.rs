use super::*;
use crate::bottom_pane::HistoryEntry;
use codex_app_server_protocol::ThreadGoalStatus;
use pretty_assertions::assert_eq;
use std::time::Instant;

const DRAFT: &str = "original draft";

fn stash(chat: &mut ChatWidget) {
    chat.apply_external_edit(DRAFT.to_string());
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert!(chat.bottom_pane.composer_is_empty());
}

fn restore(chat: &mut ChatWidget, context: &str) {
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
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
    chat.on_task_complete(Option::default(), Option::default(), from_replay);
}

#[tokio::test]
async fn manual_stash_is_lossless_non_overwriting_and_empty_stash_is_a_no_op() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    stash(&mut chat);
    chat.apply_external_edit("new draft".to_string());
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert_eq!(chat.bottom_pane.composer_text(), "new draft");
    chat.apply_external_edit(String::new());
    restore(&mut chat, "manual restore");
    chat.apply_external_edit(String::new());
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert!(chat.bottom_pane.composer_is_empty());
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_remote_image_urls(vec!["https://example.test/i".to_string()]);
    chat.bottom_pane.set_composer_text_with_mention_bindings(
        "$s [i] [p]".to_string(),
        vec![TextElement::new((0..2).into(), Some("$s".to_string()))],
        vec![PathBuf::from("/tmp/i")],
        vec![MentionBinding {
            sigil: '$',
            mention: "s".to_string(),
            path: "skill:///s".to_string(),
        }],
    );
    chat.bottom_pane
        .set_composer_pending_pastes(vec![("[p]".into(), "p".into())]);
    chat.bottom_pane.set_composer_cursor(/*cursor*/ 3);
    let expected = chat.bottom_pane.composer_draft_snapshot();
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert_eq!(chat.bottom_pane.composer_draft_snapshot(), expected);
}

#[tokio::test]
async fn stash_restoration_is_bound_to_an_accepted_live_turn_and_idle_state() {
    let (mut idle, _op_rx) = armed_stash().await;
    handle_turn_completed(&mut idle, "turn-0", /*duration_ms*/ None);
    restore(&mut idle, "unrelated turn");
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
    handle_turn_completed(&mut goal, "turn-1", /*duration_ms*/ None);
    assert!(goal.bottom_pane.composer_is_empty(), "active goal");
    handle_turn_started(&mut goal, "turn-2");
    goal.current_goal_status = None;
    handle_turn_completed(&mut goal, "turn-2", /*duration_ms*/ None);
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
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert_eq!(chat.bottom_pane.composer_text(), "search newer");
    for event in [
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::Char('\u{0013}'), KeyModifiers::NONE),
    ] {
        let (mut chat, _sender, _rx, _op_rx) = make_chatwidget_manual_with_sender().await;
        chat.apply_external_edit("visible prefix ".to_string());
        for ch in "buffered suffix".chars() {
            chat.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        chat.handle_key_event(event);
        let stash = chat.prompt_stash.as_ref().expect("stashed prompt");
        assert_eq!(stash.composer.text, "visible prefix buffered suffix");
    }
}

#[tokio::test]
async fn live_steers_keep_stash_bound_to_the_running_turn() {
    for created in [false, true] {
        let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.thread_id = Some(ThreadId::new());
        if created {
            handle_turn_started(&mut chat, "turn-1");
            stash(&mut chat);
        } else {
            stash(&mut chat);
            chat.submit_user_message(UserMessage::from("short question"));
            let _ = next_submit_op(&mut op_rx);
            handle_turn_started(&mut chat, "turn-1");
        }
        chat.apply_external_edit("steer after stash".to_string());
        chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let _ = next_submit_op(&mut op_rx);
        handle_turn_completed(&mut chat, "turn-1", /*duration_ms*/ None);
        assert_eq!(chat.bottom_pane.composer_text(), DRAFT, "{created}");
    }
}

#[tokio::test]
async fn buffered_input_prevents_automatic_stash_restoration() {
    let (mut chat, _op_rx) = armed_stash().await;
    for ch in "new draft".chars() {
        chat.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    assert!(chat.bottom_pane.is_in_paste_burst());
    handle_turn_completed(&mut chat, "turn-1", /*duration_ms*/ None);
    chat.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(chat.bottom_pane.composer_text(), "new draft");
    chat.apply_external_edit(String::new());
    restore(&mut chat, "buffered draft non-overwrite");
}
