use super::*;
use crate::bottom_pane::HistoryEntry;
use codex_app_server_protocol::ThreadGoalStatus;
use pretty_assertions::assert_eq;
use std::time::Instant;
const DRAFT: &str = "original draft";
fn type_text(c: &mut ChatWidget, text: &str) {
    text.chars()
        .for_each(|ch| c.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)));
}
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
    let (mut rich, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    rich.set_remote_image_urls(vec!["https://example.test/i".to_string()]);
    let pane = &mut rich.bottom_pane;
    pane.set_composer_text_with_mention_bindings(
        "$s [i] [p]".to_string(),
        vec![TextElement::new((0..2).into(), Some("$s".to_string()))],
        vec![PathBuf::from("/tmp/i")],
        vec![MentionBinding {
            sigil: '$',
            mention: "s".into(),
            path: "skill:///s".into(),
        }],
    );
    pane.set_composer_pending_pastes(vec![("[p]".into(), "p".into())]);
    pane.set_composer_cursor(/*cursor*/ 3);
    let expected = pane.composer_draft_snapshot();
    let stash = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
    rich.handle_key_event(stash);
    rich.handle_key_event(stash);
    assert_eq!(rich.bottom_pane.composer_draft_snapshot(), expected);
    let raw_c0 = KeyEvent::new(KeyCode::Char('\u{0013}'), KeyModifiers::NONE);
    for (event, remap) in [
        (raw_c0, false),
        (KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE), true),
    ] {
        let (mut chat, _sender, _rx, _op_rx) = make_chatwidget_manual_with_sender().await;
        if remap {
            chat.chat_keymap.stash_prompt = vec![crate::key_hint::plain(KeyCode::Char('z'))];
        }
        chat.apply_external_edit("visible prefix ".to_string());
        type_text(&mut chat, "buffered suffix");
        chat.handle_key_event(event);
        let expected = "visible prefix buffered suffix";
        let stash = chat.prompt_stash.as_ref().expect("stashed prompt");
        assert_eq!(stash.composer.text, expected);
        chat.handle_key_event(event);
        assert_eq!(chat.bottom_pane.composer_text(), expected);
    }
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
    let status = status_and_layout::test_thread_goal(ThreadGoalStatus::Active, None, 0);
    goal.current_goal_status = Some(GoalStatusState::new(status, Instant::now()));
    handle_turn_completed(&mut goal, "turn-1", /*duration_ms*/ None);
    assert!(goal.bottom_pane.composer_is_empty(), "active goal");
    handle_turn_started(&mut goal, "turn-2");
    goal.current_goal_status = None;
    handle_turn_completed(&mut goal, "turn-2", /*duration_ms*/ None);
    assert_eq!(goal.bottom_pane.composer_text(), DRAFT);
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    for entry in ["search older", "search newer"] {
        chat.bottom_pane
            .record_replayed_user_message_history(HistoryEntry::new(entry.into()));
    }
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    type_text(&mut chat, "search");
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert_eq!(chat.bottom_pane.composer_text(), "search older");
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert_eq!(chat.bottom_pane.composer_text(), "search newer");
    let (mut replayed, _op_rx) = armed_stash().await;
    replayed.turn_lifecycle.last_turn_id = Some("turn-0".to_string());
    replayed.on_task_complete(None, None, /*from_replay*/ true);
    restore(&mut replayed, "replayed completion");
    for (context, message, reject) in [
        ("local shell", "!pwd", false),
        ("rejected model turn", "rejected question", true),
    ] {
        let (mut chat, _rx, op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.thread_id = Some(ThreadId::new());
        stash(&mut chat);
        let _op_rx = (!reject).then_some(op_rx);
        chat.submit_user_message(UserMessage::from(message));
        chat.on_task_complete(None, None, /*from_replay*/ false);
        handle_turn_started(&mut chat, "unrelated-turn");
        handle_turn_completed(&mut chat, "unrelated-turn", /*duration_ms*/ None);
        assert!(chat.bottom_pane.composer_is_empty(), "{context}");
        restore(&mut chat, context);
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
    let (mut chat, _op_rx) = armed_stash().await;
    type_text(&mut chat, "new draft");
    assert!(chat.bottom_pane.is_in_paste_burst());
    handle_turn_completed(&mut chat, "turn-1", /*duration_ms*/ None);
    chat.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(chat.bottom_pane.composer_text(), "new draft");
    chat.apply_external_edit(String::new());
    restore(&mut chat, "buffered draft non-overwrite");
}
