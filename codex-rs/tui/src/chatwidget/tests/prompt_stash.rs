use super::*;
use crate::bottom_pane::HistoryEntry;
use codex_app_server_protocol::ThreadGoal as AppThreadGoal;
use codex_app_server_protocol::ThreadGoalStatus as AppThreadGoalStatus;
use pretty_assertions::assert_eq;
use std::time::Instant;

const STASHED_DRAFT: &str = "original draft";

fn press_stash(chat: &mut ChatWidget) {
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
}

fn set_composer_text(chat: &mut ChatWidget, text: &str) {
    chat.bottom_pane
        .set_composer_text(text.to_string(), Vec::new(), Vec::new());
}

fn assert_composer_text(chat: &ChatWidget, expected: &str) {
    assert_eq!(chat.bottom_pane.composer_text(), expected);
}

fn stash_draft(chat: &mut ChatWidget) {
    set_composer_text(chat, STASHED_DRAFT);
    press_stash(chat);
    assert!(chat.bottom_pane.composer_is_empty());
}

fn complete_task(chat: &mut ChatWidget, from_replay: bool) {
    chat.on_task_complete(
        /*last_agent_message*/ None,
        /*duration_ms*/ None,
        from_replay,
    );
}

fn assert_manual_restore(chat: &mut ChatWidget) {
    press_stash(chat);
    assert_composer_text(chat, STASHED_DRAFT);
}

async fn armed_stash() -> (ChatWidget, tokio::sync::mpsc::UnboundedReceiver<Op>) {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    stash_draft(&mut chat);
    set_composer_text(&mut chat, "short question");
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let _ = next_submit_op(&mut op_rx);
    handle_turn_started(&mut chat, "turn-1");
    (chat, op_rx)
}

#[tokio::test]
async fn manual_stash_restore_is_non_overwriting_and_empty_stash_is_a_no_op() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    stash_draft(&mut chat);
    set_composer_text(&mut chat, "new draft");
    press_stash(&mut chat);
    assert_composer_text(&chat, "new draft");
    set_composer_text(&mut chat, "");
    assert_manual_restore(&mut chat);
    set_composer_text(&mut chat, "");
    press_stash(&mut chat);
    assert!(chat.bottom_pane.composer_is_empty());
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn stash_round_trip_preserves_rich_draft_and_cursor() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let mention = "$skill";
    let image = "[Image #1]";
    let paste = "[Pasted Content 5001 chars]";
    let text = format!("lead {mention} {image} {paste} tail");
    let text_elements = [mention, image, paste]
        .into_iter()
        .map(|placeholder| {
            let start = text.find(placeholder).expect("placeholder in draft");
            TextElement::new(
                (start..start + placeholder.len()).into(),
                Some(placeholder.to_string()),
            )
        })
        .collect();
    chat.set_remote_image_urls(vec!["https://example.com/stashed-remote.png".to_string()]);
    chat.bottom_pane.set_composer_text_with_mention_bindings(
        text,
        text_elements,
        vec![PathBuf::from("/tmp/stashed-local.png")],
        vec![MentionBinding {
            sigil: '$',
            mention: "skill".to_string(),
            path: "skill:///tmp/SKILL.md".to_string(),
        }],
    );
    chat.bottom_pane
        .set_composer_pending_pastes(vec![(paste.to_string(), "x".repeat(5_001))]);
    chat.bottom_pane.set_composer_cursor(/*cursor*/ 3);
    let expected = chat.bottom_pane.composer_draft_snapshot();
    press_stash(&mut chat);
    press_stash(&mut chat);
    assert_eq!(chat.bottom_pane.composer_draft_snapshot(), expected);
}

#[tokio::test]
async fn live_completion_restores_only_when_the_composer_is_empty() {
    let (mut idle, _op_rx) = armed_stash().await;
    handle_turn_completed(&mut idle, "turn-1", /*duration_ms*/ None);
    assert_composer_text(&idle, STASHED_DRAFT);
    let (mut busy, _op_rx) = armed_stash().await;
    set_composer_text(&mut busy, "new draft");
    handle_turn_completed(&mut busy, "turn-1", /*duration_ms*/ None);
    assert_composer_text(&busy, "new draft");
    set_composer_text(&mut busy, "");
    assert_manual_restore(&mut busy);
}

#[tokio::test]
async fn unrelated_and_replayed_completions_do_not_restore_the_stash() {
    let (mut unrelated, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    unrelated.on_task_started();
    stash_draft(&mut unrelated);
    complete_task(&mut unrelated, /*from_replay*/ false);
    assert_manual_restore(&mut unrelated);
    let (mut replayed, _op_rx) = armed_stash().await;
    complete_task(&mut replayed, /*from_replay*/ true);
    assert!(replayed.bottom_pane.composer_is_empty());
    complete_task(&mut replayed, /*from_replay*/ false);
    assert_composer_text(&replayed, STASHED_DRAFT);
}

#[tokio::test]
async fn automatic_restore_waits_for_follow_up_queue_and_active_goal() {
    let (mut queued, mut op_rx) = armed_stash().await;
    queued.queue_user_message(UserMessage::from("queued follow-up"));
    handle_turn_completed(&mut queued, "turn-1", /*duration_ms*/ None);
    let _ = next_submit_op(&mut op_rx);
    assert!(queued.bottom_pane.composer_is_empty(), "queued follow-up");
    handle_turn_started(&mut queued, "turn-2");
    handle_turn_completed(&mut queued, "turn-2", /*duration_ms*/ None);
    assert_composer_text(&queued, STASHED_DRAFT);
    let (mut goal, _op_rx) = armed_stash().await;
    let thread_id = goal.thread_id.expect("thread id");
    goal.current_goal_status = Some(GoalStatusState::new(
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
    complete_task(&mut goal, /*from_replay*/ false);
    assert!(goal.bottom_pane.composer_is_empty(), "active goal");
    goal.current_goal_status = None;
    complete_task(&mut goal, /*from_replay*/ false);
    assert_composer_text(&goal, STASHED_DRAFT);
}

#[tokio::test]
async fn shell_and_rejected_submissions_do_not_arm_the_stash() {
    for (context, message, reject) in [
        ("local shell", "!pwd", false),
        ("rejected model turn", "rejected question", true),
    ] {
        let (mut chat, _rx, op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.thread_id = Some(ThreadId::new());
        stash_draft(&mut chat);
        if reject {
            drop(op_rx);
        }
        chat.submit_user_message(UserMessage::from(message));
        complete_task(&mut chat, /*from_replay*/ false);
        assert!(chat.bottom_pane.composer_is_empty(), "{context}");
        assert_manual_restore(&mut chat);
    }
}

#[tokio::test]
async fn history_search_keeps_ctrl_s_forward_navigation() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    for entry in ["search older", "search newer"] {
        chat.bottom_pane
            .record_replayed_user_message_history(HistoryEntry::new(entry.to_string()));
    }
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    for ch in "search".chars() {
        chat.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    assert_composer_text(&chat, "search newer");
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert_composer_text(&chat, "search older");
    press_stash(&mut chat);
    assert!(chat.bottom_pane.composer_history_search_active());
    assert_composer_text(&chat, "search newer");
}

#[tokio::test]
async fn stash_flushes_paste_burst_for_both_ctrl_s_encodings() {
    for stash_key in [
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::Char('\u{0013}'), KeyModifiers::NONE),
    ] {
        let (mut chat, _sender, _rx, _op_rx) = make_chatwidget_manual_with_sender().await;
        set_composer_text(&mut chat, "visible prefix ");
        for ch in "buffered suffix".chars() {
            chat.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert!(chat.bottom_pane.is_in_paste_burst());
        assert_composer_text(&chat, "visible prefix ");
        chat.handle_key_event(stash_key);
        assert!(!chat.bottom_pane.is_in_paste_burst());
        assert!(chat.bottom_pane.composer_is_empty());
        let stash = chat
            .prompt_stash
            .as_ref()
            .expect("prompt should be stashed");
        assert_eq!(stash.composer.text, "visible prefix buffered suffix");
        chat.handle_key_event(stash_key);
        assert_composer_text(&chat, "visible prefix buffered suffix");
    }
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
    assert_composer_text(&chat, "thread draft");
}
