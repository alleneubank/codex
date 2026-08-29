use std::collections::VecDeque;

use pretty_assertions::assert_eq;

use super::*;

#[derive(Clone, Copy, Debug)]
enum ConfigurablePlainUpAction {
    Reasoning,
    Permission,
    Copy,
}

fn bind_configurable_plain_up(chat: &mut ChatWidget, action: ConfigurablePlainUpAction) {
    let binding = vec![crate::key_hint::plain(KeyCode::Up)];
    match action {
        ConfigurablePlainUpAction::Reasoning => {
            chat.chat_keymap.increase_reasoning_effort = binding;
            chat.set_reasoning_effort(Some(ReasoningEffortConfig::Medium));
        }
        ConfigurablePlainUpAction::Permission => {
            chat.chat_keymap.next_permission_mode = binding;
        }
        ConfigurablePlainUpAction::Copy => chat.copy_last_response_binding = binding,
    }
}

fn configurable_action_ran(action: ConfigurablePlainUpAction, events: &[AppEvent]) -> bool {
    events.iter().any(|event| match action {
        ConfigurablePlainUpAction::Reasoning => {
            matches!(event, AppEvent::UpdateReasoningEffort(_))
        }
        ConfigurablePlainUpAction::Permission => matches!(
            event,
            AppEvent::ApplyPermissionShortcut { .. }
                | AppEvent::OpenFullAccessConfirmation { .. }
                | AppEvent::InsertHistoryCell(_)
        ),
        ConfigurablePlainUpAction::Copy => matches!(event, AppEvent::InsertHistoryCell(_)),
    })
}

#[tokio::test]
async fn plain_up_requests_the_newest_identical_accepted_pending_steer() {
    let (mut chat, _events, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    chat.input_queue.pending_steers = VecDeque::from([
        pending("client-older", "turn-older", "identical"),
        pending("client-newest", "turn-newest", "identical"),
    ]);

    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

    let Op::WithdrawPendingSteer {
        source_thread_id,
        accepted_turn_id,
        client_user_message_id,
        request_id,
    } = op_rx
        .try_recv()
        .expect("plain Up should request withdrawal")
    else {
        unreachable!()
    };
    assert_eq!(
        (source_thread_id, accepted_turn_id, client_user_message_id),
        (
            thread_id,
            "turn-newest".to_string(),
            "client-newest".to_string()
        )
    );
    assert_eq!(
        chat.input_queue
            .pending_steers
            .iter()
            .map(|pending| pending.lifecycle.clone())
            .collect::<Vec<_>>(),
        vec![
            PendingSteerLifecycle::Accepted {
                turn_id: "turn-older".to_string(),
            },
            PendingSteerLifecycle::WithdrawalInFlight {
                accepted_turn_id: "turn-newest".to_string(),
                request_id,
            },
        ]
    );
    assert!(
        op_rx.try_recv().is_err(),
        "plain Up must not interrupt or submit a fallback turn"
    );
}

#[tokio::test]
async fn pending_steer_edit_wins_a_plain_up_queued_edit_binding_collision() {
    let (mut chat, _events, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    chat.chat_keymap.edit_queued_message = vec![crate::key_hint::plain(KeyCode::Up)];
    chat.input_queue.pending_steers =
        VecDeque::from([pending("client-pending", "turn-pending", "pending")]);
    chat.input_queue
        .queued_user_messages
        .push_back(UserMessage::from("queued").into());
    let queued_before = chat.input_queue.queued_user_messages.clone();

    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

    assert!(matches!(
        op_rx.try_recv(),
        Ok(Op::WithdrawPendingSteer {
            source_thread_id,
            accepted_turn_id,
            client_user_message_id,
            ..
        }) if source_thread_id == thread_id
            && accepted_turn_id == "turn-pending"
            && client_user_message_id == "client-pending"
    ));
    assert_eq!(chat.input_queue.queued_user_messages, queued_before);
    assert!(chat.bottom_pane.composer_is_empty());
}

#[tokio::test]
async fn pending_steer_edit_wins_a_plain_up_stash_binding_collision() {
    let (mut chat, _events, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    chat.chat_keymap.stash_prompt = vec![crate::key_hint::plain(KeyCode::Up)];
    chat.input_queue.pending_steers =
        VecDeque::from([pending("client-pending", "turn-pending", "pending")]);

    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

    assert!(matches!(
        op_rx.try_recv(),
        Ok(Op::WithdrawPendingSteer {
            accepted_turn_id,
            client_user_message_id,
            ..
        }) if accepted_turn_id == "turn-pending"
            && client_user_message_id == "client-pending"
    ));
    assert!(chat.prompt_stash.is_none());
}

#[tokio::test]
async fn pending_steer_edit_wins_earlier_configurable_plain_up_collisions() {
    for action in [
        ConfigurablePlainUpAction::Reasoning,
        ConfigurablePlainUpAction::Permission,
        ConfigurablePlainUpAction::Copy,
    ] {
        let (mut chat, mut events, mut op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
        chat.thread_id = Some(ThreadId::new());
        bind_configurable_plain_up(&mut chat, action);
        while events.try_recv().is_ok() {}
        chat.input_queue.pending_steers =
            VecDeque::from([pending("client-pending", "turn-pending", "pending")]);

        chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

        assert!(
            matches!(
                op_rx.try_recv(),
                Ok(Op::WithdrawPendingSteer {
                    accepted_turn_id,
                    client_user_message_id,
                    ..
                }) if accepted_turn_id == "turn-pending"
                    && client_user_message_id == "client-pending"
            ),
            "{action:?}"
        );
        let routed_events = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
        assert!(
            !configurable_action_ran(action, &routed_events),
            "{action:?}"
        );
        assert!(!chat.permission_shortcut_pending, "{action:?}");
    }
}

#[tokio::test]
async fn earlier_configurable_plain_up_actions_run_without_an_eligible_pending_steer() {
    for action in [
        ConfigurablePlainUpAction::Reasoning,
        ConfigurablePlainUpAction::Permission,
        ConfigurablePlainUpAction::Copy,
    ] {
        let (mut chat, mut events, mut op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
        chat.thread_id = Some(ThreadId::new());
        bind_configurable_plain_up(&mut chat, action);
        while events.try_recv().is_ok() {}
        let mut awaiting = pending("client-awaiting", "turn-awaiting", "awaiting");
        awaiting.lifecycle = PendingSteerLifecycle::AwaitingAcceptance;
        chat.input_queue.pending_steers = VecDeque::from([awaiting]);
        let pending_before = chat.input_queue.pending_steers.clone();

        chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

        let routed_events = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
        assert!(
            configurable_action_ran(action, &routed_events),
            "{action:?}"
        );
        assert_eq!(
            chat.input_queue.pending_steers, pending_before,
            "{action:?}"
        );
        assert!(op_rx.try_recv().is_err(), "{action:?}");
        chat.permission_shortcut_pending = false;
    }
}

#[tokio::test]
async fn plain_up_queued_edit_binding_still_runs_without_an_eligible_pending_steer() {
    let (mut chat, _events, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    chat.chat_keymap.edit_queued_message = vec![crate::key_hint::plain(KeyCode::Up)];
    let mut awaiting = pending("client-awaiting", "turn-awaiting", "awaiting");
    awaiting.lifecycle = PendingSteerLifecycle::AwaitingAcceptance;
    chat.input_queue.pending_steers = VecDeque::from([awaiting]);
    let pending_before = chat.input_queue.pending_steers.clone();
    chat.input_queue
        .queued_user_messages
        .push_back(UserMessage::from("queued").into());

    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

    assert_eq!(chat.input_queue.pending_steers, pending_before);
    assert!(chat.input_queue.queued_user_messages.is_empty());
    assert_eq!(chat.bottom_pane.composer_text(), "queued");
    assert!(op_rx.try_recv().is_err());
}

#[tokio::test]
async fn empty_composer_plain_up_recalls_history_without_an_eligible_pending_steer() {
    let (mut chat, _events, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    let mut awaiting = pending("client-awaiting", "turn-awaiting", "awaiting");
    awaiting.lifecycle = PendingSteerLifecycle::AwaitingAcceptance;
    chat.input_queue.pending_steers = VecDeque::from([awaiting]);
    replay_user_message_text(
        &mut chat,
        "history-item",
        "recalled from history",
        ReplayKind::ResumeInitialMessages,
    );
    let pending_before = chat.input_queue.pending_steers.clone();

    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

    assert_eq!(chat.bottom_pane.composer_text(), "recalled from history");
    assert_eq!(chat.input_queue.pending_steers, pending_before);
    assert!(op_rx.try_recv().is_err());
}

#[tokio::test]
async fn plain_up_preserves_ineligible_composer_and_modal_behavior() {
    for setup in ["nonempty", "awaiting-acceptance", "modal"] {
        let (mut chat, _events, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.thread_id = Some(ThreadId::new());
        chat.input_queue.pending_steers =
            VecDeque::from([pending("client-pending", "turn-pending", "pending")]);
        match setup {
            "nonempty" => chat.bottom_pane.set_composer_text(
                "existing draft".to_string(),
                Vec::new(),
                Vec::new(),
            ),
            "awaiting-acceptance" => {
                chat.input_queue.pending_steers[0].lifecycle =
                    PendingSteerLifecycle::AwaitingAcceptance;
            }
            "modal" => chat.show_selection_view(SelectionViewParams {
                view_id: Some("pending-steer-edit-test"),
                items: vec![SelectionItem {
                    name: "Choice".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            _ => unreachable!(),
        }
        let before = chat.input_queue.pending_steers.clone();

        chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

        assert_eq!(chat.input_queue.pending_steers, before, "{setup}");
        assert!(op_rx.try_recv().is_err(), "{setup}");
        if setup == "nonempty" {
            assert_eq!(chat.bottom_pane.composer_text(), "existing draft");
        }
    }
}

#[tokio::test]
async fn nonempty_composer_plain_up_preserves_multiline_cursor_navigation() {
    let (mut chat, _events, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    chat.input_queue.pending_steers =
        VecDeque::from([pending("client-pending", "turn-pending", "pending")]);
    let text = "first line\nsecond line".to_string();
    chat.bottom_pane
        .set_composer_text(text.clone(), Vec::new(), Vec::new());
    chat.bottom_pane.set_composer_cursor(text.len());

    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

    assert_eq!(chat.bottom_pane.composer_text(), text);
    assert_eq!(chat.bottom_pane.composer_cursor(), "first line".len());
    assert!(op_rx.try_recv().is_err());
}

#[tokio::test]
async fn modal_plain_up_changes_the_modal_selection() {
    let (mut chat, mut events, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    chat.input_queue.pending_steers =
        VecDeque::from([pending("client-pending", "turn-pending", "pending")]);
    chat.show_selection_view(SelectionViewParams {
        view_id: Some("pending-steer-edit-modal-routing"),
        items: vec![
            SelectionItem {
                name: "First".to_string(),
                ..Default::default()
            },
            SelectionItem {
                name: "Second".to_string(),
                ..Default::default()
            },
        ],
        on_selection_changed: Some(Box::new(|index, tx| {
            tx.send(AppEvent::InsertHistoryCell(Box::new(
                history_cell::new_info_event(format!("selected {index}"), /*hint*/ None),
            )));
        })),
        ..Default::default()
    });
    chat.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    while events.try_recv().is_ok() {}

    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

    assert!(matches!(
        events.try_recv(),
        Ok(AppEvent::InsertHistoryCell(_))
    ));
    assert!(op_rx.try_recv().is_err());
}

#[tokio::test]
async fn paste_burst_plain_up_flushes_the_buffer_without_withdrawing() {
    let (mut chat, _events, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    chat.input_queue.pending_steers =
        VecDeque::from([pending("client-pending", "turn-pending", "pending")]);
    let pending_before = chat.input_queue.pending_steers.clone();
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    assert!(chat.bottom_pane.composer_is_empty());
    assert!(chat.bottom_pane.is_in_paste_burst());

    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

    assert_eq!(chat.bottom_pane.composer_text(), "h");
    assert!(!chat.bottom_pane.is_in_paste_burst());
    assert_eq!(chat.input_queue.pending_steers, pending_before);
    assert!(op_rx.try_recv().is_err());
}

#[tokio::test]
async fn repeated_plain_up_is_consumed_while_withdrawal_is_unresolved() {
    for lifecycle in [
        PendingSteerLifecycle::WithdrawalInFlight {
            accepted_turn_id: "turn-pending".to_string(),
            request_id: "request-pending".to_string(),
        },
        PendingSteerLifecycle::WithdrawalUncertain {
            accepted_turn_id: "turn-pending".to_string(),
            request_id: "request-pending".to_string(),
        },
    ] {
        let (mut chat, _events, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.thread_id = Some(ThreadId::new());
        let mut row = pending("client-pending", "turn-pending", "pending");
        row.lifecycle = lifecycle;
        chat.input_queue.pending_steers = VecDeque::from([row]);
        let before = chat.input_queue.pending_steers.clone();

        assert!(
            chat.handle_pending_steer_edit_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE,))
        );
        assert!(
            chat.handle_pending_steer_edit_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE,))
        );

        assert_eq!(chat.input_queue.pending_steers, before);
        assert!(op_rx.try_recv().is_err());
    }
}

#[tokio::test]
async fn repeated_key_event_up_is_consumed_while_withdrawal_is_unresolved() {
    let (mut chat, mut events, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    let mut row = pending("client-pending", "turn-pending", "pending");
    row.lifecycle = PendingSteerLifecycle::WithdrawalInFlight {
        accepted_turn_id: "turn-pending".to_string(),
        request_id: "request-pending".to_string(),
    };
    chat.input_queue.pending_steers = VecDeque::from([row]);
    let before = chat.input_queue.pending_steers.clone();
    while events.try_recv().is_ok() {}
    let mut repeat = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
    repeat.kind = KeyEventKind::Repeat;

    assert!(chat.handle_pending_steer_edit_key(repeat));
    chat.handle_key_event(repeat);

    assert_eq!(chat.input_queue.pending_steers, before);
    assert!(chat.bottom_pane.composer_is_empty());
    assert!(op_rx.try_recv().is_err());
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn commit_before_withdrawal_response_makes_late_success_a_noop() {
    let (mut chat, _events, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    chat.input_queue.pending_steers =
        VecDeque::from([pending("client-pending", "turn-pending", "pending")]);

    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    let Op::WithdrawPendingSteer {
        accepted_turn_id,
        client_user_message_id,
        request_id,
        ..
    } = op_rx.try_recv().expect("withdrawal request")
    else {
        unreachable!()
    };
    complete_user_message_with_client_id(
        &mut chat,
        "item-pending",
        Some(&client_user_message_id),
        vec![UserInput::Text {
            text: "pending".to_string(),
            text_elements: Vec::new(),
        }],
    );

    assert_eq!(
        chat.reconcile_pending_steer_withdrawal(
            &client_user_message_id,
            &accepted_turn_id,
            &request_id,
            PendingSteerWithdrawalOutcome::Withdrawn,
        ),
        PendingSteerWithdrawalEffect::Noop
    );
    assert!(chat.input_queue.pending_steers.is_empty());
    assert!(chat.bottom_pane.composer_is_empty());
}

#[tokio::test]
async fn confirmed_withdrawal_restores_rich_message_and_resubmits_with_a_fresh_id() {
    let (mut chat, _events, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    let local_image = LocalImageAttachment {
        placeholder: "[Image #2]".to_string(),
        path: PathBuf::from("/tmp/pending-local.png"),
    };
    let remote_image = "https://example.test/pending.png".to_string();
    let mention = MentionBinding {
        sigil: '$',
        mention: "skill".to_string(),
        path: "/tmp/skills/skill/SKILL.md".to_string(),
    };
    let restored_text = "$skill inspect [Image #2]".to_string();
    let restored_elements = vec![TextElement::new((0..6).into(), Some("$skill".to_string()))];
    let pending = PendingSteer {
        client_user_message_id: "client-original".to_string(),
        user_message: UserMessage {
            text: "canonical submitted text".to_string(),
            local_images: vec![local_image.clone()],
            remote_image_urls: vec![remote_image.clone()],
            text_elements: Vec::new(),
            mention_bindings: vec![mention.clone()],
        },
        history_record: UserMessageHistoryRecord::Override(UserMessageHistoryOverride {
            text: restored_text.clone(),
            text_elements: restored_elements.clone(),
        }),
        lifecycle: PendingSteerLifecycle::WithdrawalInFlight {
            accepted_turn_id: "turn-original".to_string(),
            request_id: "request-original".to_string(),
        },
    };

    chat.restore_withdrawn_pending_steer(pending);

    assert_eq!(
        chat.capture_thread_input_state()
            .and_then(|state| state.composer),
        Some(ThreadComposerState {
            text: restored_text.clone(),
            local_images: vec![local_image],
            remote_image_urls: vec![remote_image],
            text_elements: restored_elements,
            mention_bindings: vec![mention],
            pending_pastes: Vec::new(),
            cursor: restored_text.len(),
        })
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let Op::UserTurn {
        client_user_message_id,
        ..
    } = next_submit_op(&mut op_rx)
    else {
        unreachable!()
    };
    assert_ne!(client_user_message_id, "client-original");
}

#[tokio::test]
async fn confirmed_withdrawal_preserves_input_entered_while_request_is_in_flight() {
    let (mut chat, _events, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.restore_composer_state(in_flight_composer_state());

    chat.restore_withdrawn_pending_steer(pending_with_attachment());

    assert_eq!(
        chat.capture_thread_input_state()
            .and_then(|state| state.composer),
        Some(expected_merged_composer_state())
    );
}

#[tokio::test]
async fn offscreen_confirmed_withdrawal_preserves_the_stored_draft() {
    let (mut chat, _events, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.restore_composer_state(in_flight_composer_state());
    let mut input_state = chat
        .capture_thread_input_state()
        .expect("thread input state should be available");

    input_state.restore_withdrawn_pending_steer(pending_with_attachment());

    assert_eq!(input_state.composer, Some(expected_merged_composer_state()));
}

#[tokio::test]
async fn event_gap_withdrawals_merge_both_confirmed_messages() {
    let (mut chat, _events, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    chat.input_queue.pending_steers = VecDeque::from([
        pending("client-older", "turn-pending", "older"),
        pending("client-newer", "turn-pending", "newer"),
    ]);

    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    let Op::WithdrawPendingSteer {
        accepted_turn_id,
        client_user_message_id,
        request_id,
        ..
    } = op_rx.try_recv().expect("newer withdrawal request")
    else {
        unreachable!()
    };
    let PendingSteerWithdrawalEffect::Withdrawn(newer) = chat.reconcile_pending_steer_withdrawal(
        &client_user_message_id,
        &accepted_turn_id,
        &request_id,
        PendingSteerWithdrawalOutcome::Withdrawn,
    ) else {
        unreachable!()
    };

    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    let Op::WithdrawPendingSteer {
        accepted_turn_id,
        client_user_message_id,
        request_id,
        ..
    } = op_rx.try_recv().expect("older withdrawal request")
    else {
        unreachable!()
    };
    chat.restore_withdrawn_pending_steer(*newer);
    let PendingSteerWithdrawalEffect::Withdrawn(older) = chat.reconcile_pending_steer_withdrawal(
        &client_user_message_id,
        &accepted_turn_id,
        &request_id,
        PendingSteerWithdrawalOutcome::Withdrawn,
    ) else {
        unreachable!()
    };
    chat.restore_withdrawn_pending_steer(*older);

    assert_eq!(chat.bottom_pane.composer_text(), "older\nnewer");
}

#[tokio::test]
async fn confirmed_withdrawal_flushes_and_merges_an_active_paste_burst() {
    let (mut chat, _events, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    chat.input_queue.pending_steers =
        VecDeque::from([pending("client-pending", "turn-pending", "pending")]);
    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    let Op::WithdrawPendingSteer {
        accepted_turn_id,
        client_user_message_id,
        request_id,
        ..
    } = op_rx.try_recv().expect("withdrawal request")
    else {
        unreachable!()
    };
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert!(chat.bottom_pane.composer_is_empty());
    assert!(chat.bottom_pane.is_in_paste_burst());
    let PendingSteerWithdrawalEffect::Withdrawn(pending) = chat.reconcile_pending_steer_withdrawal(
        &client_user_message_id,
        &accepted_turn_id,
        &request_id,
        PendingSteerWithdrawalOutcome::Withdrawn,
    ) else {
        unreachable!()
    };

    chat.restore_withdrawn_pending_steer(*pending);
    chat.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));

    assert!(!chat.bottom_pane.is_in_paste_burst());
    assert_eq!(chat.bottom_pane.composer_text(), "pending\nx");
}

#[tokio::test]
async fn uncertain_withdrawal_survives_completion_and_unmatched_replay() {
    let (mut chat, _events, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    let mut row = pending("client-pending", "turn-pending", "pending");
    row.lifecycle = PendingSteerLifecycle::WithdrawalUncertain {
        accepted_turn_id: "turn-pending".to_string(),
        request_id: "request-pending".to_string(),
    };
    chat.input_queue.pending_steers = VecDeque::from([row.clone()]);
    handle_turn_started(&mut chat, "turn-pending");

    handle_turn_completed(&mut chat, "turn-pending", /*duration_ms*/ None);
    replay_user_message_text(
        &mut chat,
        "unmatched-item",
        "other committed message",
        ReplayKind::ThreadSnapshot,
    );

    assert_eq!(chat.input_queue.pending_steers, VecDeque::from([row]));
    assert!(!chat.input_queue.preview().pending_steer_editable);
    assert!(chat.bottom_pane.composer_is_empty());
    assert!(chat.handle_pending_steer_edit_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE,)));
    assert!(op_rx.try_recv().is_err());
}

#[tokio::test]
async fn interruption_restores_uncertain_withdrawal_and_invalidates_late_response() {
    let (mut chat, _events, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    let mut row = pending("client-pending", "turn-pending", "pending");
    row.lifecycle = PendingSteerLifecycle::WithdrawalUncertain {
        accepted_turn_id: "turn-pending".to_string(),
        request_id: "request-pending".to_string(),
    };
    chat.input_queue.pending_steers = VecDeque::from([row]);
    handle_turn_started(&mut chat, "turn-pending");

    handle_turn_interrupted(&mut chat, "turn-pending");

    assert!(chat.input_queue.pending_steers.is_empty());
    assert_eq!(chat.bottom_pane.composer_text(), "pending");
    assert_eq!(
        chat.reconcile_pending_steer_withdrawal(
            "client-pending",
            "turn-pending",
            "request-pending",
            PendingSteerWithdrawalOutcome::Withdrawn,
        ),
        PendingSteerWithdrawalEffect::Noop
    );
    assert_eq!(chat.bottom_pane.composer_text(), "pending");
}

fn pending_with_attachment() -> PendingSteer {
    let text = "[Image #1] pending".to_string();
    let image_start = 0;
    PendingSteer {
        client_user_message_id: "client-original".to_string(),
        user_message: UserMessage {
            text,
            local_images: vec![LocalImageAttachment {
                placeholder: "[Image #1]".to_string(),
                path: PathBuf::from("/tmp/pending.png"),
            }],
            remote_image_urls: Vec::new(),
            text_elements: vec![TextElement::new(
                (image_start..image_start + "[Image #1]".len()).into(),
                Some("[Image #1]".to_string()),
            )],
            mention_bindings: Vec::new(),
        },
        history_record: UserMessageHistoryRecord::UserMessageText,
        lifecycle: PendingSteerLifecycle::WithdrawalInFlight {
            accepted_turn_id: "turn-original".to_string(),
            request_id: "request-original".to_string(),
        },
    }
}

fn in_flight_composer_state() -> ThreadComposerState {
    let text = "[Image #1] [Pasted Content 8 chars] in flight".to_string();
    let image_start = 0;
    ThreadComposerState {
        text,
        local_images: vec![LocalImageAttachment {
            placeholder: "[Image #1]".to_string(),
            path: PathBuf::from("/tmp/in-flight.png"),
        }],
        remote_image_urls: Vec::new(),
        text_elements: vec![TextElement::new(
            (image_start..image_start + "[Image #1]".len()).into(),
            Some("[Image #1]".to_string()),
        )],
        mention_bindings: Vec::new(),
        pending_pastes: vec![(
            "[Pasted Content 8 chars]".to_string(),
            "in-flight paste".to_string(),
        )],
        cursor: 4,
    }
}

fn expected_merged_composer_state() -> ThreadComposerState {
    let pending_image_start = 0;
    let draft_image_start = "[Image #1] pending\n".len();
    let text = "[Image #1] pending\n[Image #2] [Pasted Content 8 chars] in flight".to_string();
    ThreadComposerState {
        cursor: text.len(),
        text,
        local_images: vec![
            LocalImageAttachment {
                placeholder: "[Image #1]".to_string(),
                path: PathBuf::from("/tmp/pending.png"),
            },
            LocalImageAttachment {
                placeholder: "[Image #2]".to_string(),
                path: PathBuf::from("/tmp/in-flight.png"),
            },
        ],
        remote_image_urls: Vec::new(),
        text_elements: vec![
            TextElement::new(
                (pending_image_start..pending_image_start + "[Image #1]".len()).into(),
                Some("[Image #1]".to_string()),
            ),
            TextElement::new(
                (draft_image_start..draft_image_start + "[Image #2]".len()).into(),
                Some("[Image #2]".to_string()),
            ),
        ],
        mention_bindings: Vec::new(),
        pending_pastes: vec![(
            "[Pasted Content 8 chars]".to_string(),
            "in-flight paste".to_string(),
        )],
    }
}

fn pending(client_id: &str, turn_id: &str, text: &str) -> PendingSteer {
    PendingSteer {
        client_user_message_id: client_id.to_string(),
        user_message: UserMessage::from(text),
        history_record: UserMessageHistoryRecord::UserMessageText,
        lifecycle: PendingSteerLifecycle::Accepted {
            turn_id: turn_id.to_string(),
        },
    }
}
