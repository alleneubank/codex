use super::*;
use pretty_assertions::assert_eq;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exit_worktree_keep_true_keeps_managed_worktree() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let server = start_mock_server().await;
    let test = test_codex().build(&server).await?;
    init_git_repo(test.config.cwd.as_path())?;

    let enter_call_id = "enter-worktree";
    let exit_call_id = "exit-worktree";
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-keep-true" }),
            )?,
            function_call(
                exit_call_id,
                EXIT_WORKTREE_TOOL_NAME,
                json!({ "keep": true }),
            )?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "enter a worktree and exit while keeping it",
        PermissionProfile::workspace_write(),
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value =
        serde_json::from_str(&enter_output).context("enter_worktree output should be JSON")?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;
    assert!(
        Path::new(worktree_path).exists(),
        "keep=true should leave managed worktree on disk"
    );

    let exit_output = request_log
        .function_call_output_text(exit_call_id)
        .context("missing exit_worktree output")?;
    let exit_output: Value =
        serde_json::from_str(&exit_output).context("exit_worktree output should be JSON")?;
    assert_eq!(
        exit_output.get("cwd"),
        Some(&json!(test.config.cwd.to_string_lossy().to_string()))
    );
    assert_eq!(exit_output.get("keep"), Some(&json!(true)));
    assert_eq!(exit_output.get("removed"), Some(&json!(false)));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exit_worktree_keep_false_removes_clean_managed_worktree() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let server = start_mock_server().await;
    let test = test_codex().build(&server).await?;
    init_git_repo(test.config.cwd.as_path())?;

    let enter_call_id = "enter-worktree";
    let exit_call_id = "exit-worktree";
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-remove-clean" }),
            )?,
            function_call(
                exit_call_id,
                EXIT_WORKTREE_TOOL_NAME,
                json!({ "keep": false }),
            )?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "enter a worktree and exit while removing it",
        PermissionProfile::workspace_write(),
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value =
        serde_json::from_str(&enter_output).context("enter_worktree output should be JSON")?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;
    assert!(
        !Path::new(worktree_path).exists(),
        "keep=false should remove clean managed worktree from disk"
    );
    let worktree_list = run_git_for_stdout(test.config.cwd.as_path(), &["worktree", "list"])?;
    assert!(
        !worktree_list.contains(worktree_path),
        "removed worktree should not appear in git worktree list"
    );

    let exit_output = request_log
        .function_call_output_text(exit_call_id)
        .context("missing exit_worktree output")?;
    let exit_output: Value =
        serde_json::from_str(&exit_output).context("exit_worktree output should be JSON")?;
    assert_eq!(
        exit_output.get("cwd"),
        Some(&json!(test.config.cwd.to_string_lossy().to_string()))
    );
    assert_eq!(exit_output.get("keep"), Some(&json!(false)));
    assert_eq!(exit_output.get("removed"), Some(&json!(true)));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exit_worktree_keep_false_rejects_dirty_managed_worktree() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::UnifiedExec)
            .expect("test config should enable unified exec");
    });
    let test = builder.build(&server).await?;
    init_git_repo(test.config.cwd.as_path())?;

    let enter_call_id = "enter-worktree";
    let dirty_call_id = "dirty-worktree";
    let exit_call_id = "exit-worktree";
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-remove-dirty" }),
            )?,
            function_call(
                dirty_call_id,
                "exec_command",
                json!({
                    "cmd": "printf dirty > dirty.txt",
                    "yield_time_ms": 1_000_u64,
                    "max_output_tokens": 2_000_u64,
                }),
            )?,
            function_call(
                exit_call_id,
                EXIT_WORKTREE_TOOL_NAME,
                json!({ "keep": false }),
            )?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "enter a worktree, dirty it, and try to remove it while exiting",
        PermissionProfile::Disabled,
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value =
        serde_json::from_str(&enter_output).context("enter_worktree output should be JSON")?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;
    assert!(
        Path::new(worktree_path).exists(),
        "dirty managed worktree should be left in place"
    );

    let exit_output = request_log
        .function_call_output_text(exit_call_id)
        .context("missing exit_worktree output")?;
    assert!(
        exit_output.contains("worktree operation failed")
            && exit_output.contains("contains modified or untracked files"),
        "dirty removal should report git's refusal, got {exit_output:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exit_worktree_rejects_unknown_args() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(Ok(()), "exit_worktree requires a local primary environment");

    let server = start_mock_server().await;
    let test = test_codex().build(&server).await?;

    let exit_call_id = "exit-worktree";
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                exit_call_id,
                EXIT_WORKTREE_TOOL_NAME,
                json!({ "remove": true }),
            )?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "call exit_worktree with the wrong argument",
        PermissionProfile::Disabled,
    )
    .await?;

    let exit_output = request_log
        .function_call_output_text(exit_call_id)
        .context("missing exit_worktree output")?;
    assert!(
        exit_output.contains("unknown field") && exit_output.contains("remove"),
        "unknown argument should be rejected, got {exit_output:?}"
    );

    Ok(())
}
