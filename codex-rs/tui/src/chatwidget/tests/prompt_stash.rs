use super::*;
use crate::bottom_pane::HistoryEntry;
use pretty_assertions::assert_eq;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;

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

#[tokio::test]
async fn accepted_prompt_queue_shell_and_command_restore_stash_immediately() {
    let (mut prompt, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    prompt.thread_id = Some(ThreadId::new());
    stash(&mut prompt);
    prompt.apply_external_edit("one-off prompt".to_string());

    prompt.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let _ = next_submit_op(&mut op_rx);
    assert_eq!(prompt.bottom_pane.composer_text(), DRAFT);
    handle_turn_started(&mut prompt, "turn-1");
    assert_chatwidget_snapshot!(
        "prompt_stash_restored_while_turn_running",
        render_bottom_popup(&prompt, /*width*/ 80),
    );
    prompt.apply_external_edit("edited restored draft".to_string());
    handle_turn_completed(&mut prompt, "turn-1", /*duration_ms*/ None);
    assert_eq!(prompt.bottom_pane.composer_text(), "edited restored draft");

    let (mut queued, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    handle_turn_started(&mut queued, "turn-1");
    stash(&mut queued);
    queued.apply_external_edit("queued follow-up".to_string());

    queued.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(queued.bottom_pane.composer_text(), DRAFT);
    assert_eq!(queued.queued_user_message_texts(), vec!["queued follow-up"]);

    let (mut shell, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    shell.thread_id = Some(ThreadId::new());
    stash(&mut shell);
    shell.apply_external_edit("!pwd".to_string());

    shell.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(
        op_rx.try_recv(),
        Ok(Op::RunUserShellCommand { command }) if command == "pwd"
    );
    assert_eq!(shell.bottom_pane.composer_text(), DRAFT);

    let (mut command, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    stash(&mut command);
    command.apply_external_edit("/status".to_string());

    command.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(command.bottom_pane.composer_text(), DRAFT);
}

#[tokio::test]
async fn rejected_prompt_keeps_intervening_input_and_stash() {
    let (mut chat, _rx, op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    stash(&mut chat);
    chat.apply_external_edit("rejected question".to_string());
    drop(op_rx);

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(chat.bottom_pane.composer_text(), "rejected question");
    chat.apply_external_edit(String::new());
    restore(&mut chat, "rejected prompt");
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
async fn turn_events_and_history_search_do_not_restore_manual_stash() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    stash(&mut chat);
    handle_turn_started(&mut chat, "turn-1");
    handle_turn_completed(&mut chat, "turn-1", /*duration_ms*/ None);
    chat.on_task_complete(None, None, /*from_replay*/ true);
    assert!(chat.bottom_pane.composer_is_empty());
    restore(&mut chat, "turn notifications");

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
}

#[tokio::test]
async fn accepted_live_steer_restores_stash_immediately() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");
    stash(&mut chat);
    chat.apply_external_edit("steer after stash".to_string());

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let _ = next_submit_op(&mut op_rx);
    assert_eq!(chat.bottom_pane.composer_text(), DRAFT);
    handle_turn_completed(&mut chat, "turn-1", /*duration_ms*/ None);
    assert_eq!(chat.bottom_pane.composer_text(), DRAFT);
}

#[tokio::test]
async fn prompt_stash_indicator_tracks_presence_and_composer_layout() {
    fn render_with_metadata(
        chat: &mut ChatWidget,
        width: u16,
    ) -> (String, Buffer, Option<(u16, u16)>, u16) {
        let height = chat.desired_height(width);
        let area = Rect::new(0, 0, width, height);
        let cursor = chat.cursor_pos(area);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| chat.render(frame.area(), frame.buffer_mut()))
            .expect("draw prompt stash indicator");
        (
            normalized_backend_snapshot(terminal.backend()),
            terminal.backend().buffer().clone(),
            cursor,
            height,
        )
    }

    fn render(chat: &mut ChatWidget, width: u16) -> String {
        render_with_metadata(chat, width).0
    }

    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.bottom_pane
        .set_custom_status_line(Some("custom status".into()), /*padding*/ 0);
    stash(&mut chat);
    type_text(
        &mut chat,
        "replacement draft that should retain its existing textarea wrapping while stashed",
    );
    // A non-character key flushes paste-burst buffering before the render captures the draft.
    chat.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(
        chat.bottom_pane.composer_text(),
        "replacement draft that should retain its existing textarea wrapping while stashed"
    );

    let (wide, wide_buffer, cursor_with_indicator, height_with_indicator) =
        render_with_metadata(&mut chat, /*width*/ 80);
    assert!(wide.contains("draft stashed"));
    assert!(wide.contains("replacement draft that should retain its existing textarea"));
    assert!(wide.contains("custom status"));
    assert_chatwidget_snapshot!("prompt_stash_indicator_wide", wide);

    let indicator_position = (wide_buffer.area.y..wide_buffer.area.bottom()).find_map(|y| {
        let row = (wide_buffer.area.x..wide_buffer.area.right())
            .map(|x| wide_buffer[(x, y)].symbol().chars().next().unwrap_or(' '))
            .collect::<String>();
        row.find("draft stashed")
            .map(|x| (wide_buffer.area.x + x as u16, y))
    });
    let (indicator_x, indicator_y) = indicator_position.expect("styled stash indicator in buffer");
    for x in indicator_x..indicator_x + "draft stashed".len() as u16 {
        let style = wide_buffer[(x, indicator_y)].style();
        assert_eq!(style.fg, Some(Color::Cyan));
        assert!(style.add_modifier.contains(Modifier::DIM));
    }

    chat.bottom_pane
        .set_has_stashed_draft(/*has_stashed_draft*/ false);
    let (without_indicator, _, cursor_without_indicator, height_without_indicator) =
        render_with_metadata(&mut chat, /*width*/ 80);
    assert!(!without_indicator.contains("draft stashed"));
    assert_eq!(
        (height_with_indicator, cursor_with_indicator),
        (height_without_indicator, cursor_without_indicator),
        "the presentation projection does not change composer geometry or cursor placement"
    );
    chat.bottom_pane
        .set_has_stashed_draft(/*has_stashed_draft*/ true);

    chat.set_pet_image_support_for_tests(crate::pets::PetImageSupport::Supported(
        crate::pets::ImageProtocol::Kitty,
    ));
    chat.install_test_ambient_pet_for_tests(/*animations_enabled*/ false);
    let pet_reserve = chat.ambient_pet_wrap_reserved_cols();
    assert!(
        pet_reserve > 0,
        "the supported ambient pet reserves composer columns"
    );
    let reserved_snapshot = render(&mut chat, /*width*/ 80);
    let indicator_line = reserved_snapshot
        .lines()
        .find(|line| line.contains("draft stashed"))
        .expect("full-widget render contains the stash indicator")
        .trim_matches('"');
    let indicator_end = indicator_line
        .find("draft stashed")
        .expect("indicator line contains the full label")
        + "draft stashed".len();
    assert_eq!(
        indicator_line[indicator_end..].chars().count(),
        usize::from(pet_reserve.saturating_add(1)),
        "the indicator stays outside the ambient pet reservation and textarea trailing column"
    );

    // User text must not be able to satisfy the label boundary assertions.
    chat.apply_external_edit(String::new());
    assert!(render(&mut chat, pet_reserve + 14).contains("draft stashed"));
    let compact_boundary = render(&mut chat, pet_reserve + 13);
    assert!(compact_boundary.contains("stashed"));
    assert!(!compact_boundary.contains("draft stashed"));
    assert!(render(&mut chat, pet_reserve + 8).contains("stashed"));
    assert!(!render(&mut chat, pet_reserve + 7).contains("stashed"));
    chat.disable_ambient_pet_for_session();

    let compact = render(&mut chat, /*width*/ 12);
    assert!(compact.contains("stashed"));
    assert!(!compact.contains("draft stashed"));
    assert_chatwidget_snapshot!("prompt_stash_indicator_compact", compact);

    let omitted = render(&mut chat, /*width*/ 6);
    assert!(!omitted.contains("stashed"));
    assert_chatwidget_snapshot!("prompt_stash_indicator_omitted", omitted);

    restore(&mut chat, "indicator clears with manual restore");
    assert!(!render(&mut chat, /*width*/ 80).contains("stashed"));

    let (mut accepted, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    accepted.thread_id = Some(ThreadId::new());
    stash(&mut accepted);
    accepted.apply_external_edit("one-off prompt".to_string());
    accepted.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let _ = next_submit_op(&mut op_rx);
    assert_eq!(accepted.bottom_pane.composer_text(), DRAFT);
    assert!(
        !render(&mut accepted, /*width*/ 80).contains("stashed"),
        "accepted-submission restoration clears the indicator"
    );

    let (mut source, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    stash(&mut source);
    let stashed_input = source.capture_thread_input_state();
    let (mut restored, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    restored.restore_thread_input_state(stashed_input);
    assert!(
        render(&mut restored, /*width*/ 80).contains("draft stashed"),
        "restoring thread input with a stash projects the indicator"
    );
    restored.restore_thread_input_state(/*input_state*/ None);
    assert!(
        !render(&mut restored, /*width*/ 80).contains("stashed"),
        "restoring thread input without a stash clears the indicator"
    );
}
