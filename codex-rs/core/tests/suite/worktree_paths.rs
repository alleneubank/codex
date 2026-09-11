use super::*;
use pretty_assertions::assert_eq;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_worktree_preserves_relative_subdirectory_cwd() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let fixture = tempfile::TempDir::new()?;
    let repo = fixture.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_git_repo(&repo)?;
    let subdir = commit_tracked_subdir(&repo, "codex-rs")?;

    let subdir = dunce::canonicalize(subdir)?.abs();
    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(move |config| {
        config.cwd = subdir;
        config.workspace_roots = vec![config.cwd.clone()];
        config
            .features
            .enable(Feature::UnifiedExec)
            .expect("test config should enable unified exec");
    });
    let test = builder.build(&server).await?;

    let enter_call_id = "enter-subdir-worktree";
    let pwd_call_id = "pwd-in-subdir-worktree";
    let exit_call_id = "exit-subdir-worktree";
    let original_pwd_call_id = "pwd-after-subdir-exit";
    let pwd_args = json!({
        "cmd": "pwd",
        "yield_time_ms": 1_000_u64,
        "max_output_tokens": 2_000_u64,
    });
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-subdir-cwd" }),
            )?,
            function_call(pwd_call_id, "exec_command", pwd_args.clone())?,
            function_call(exit_call_id, EXIT_WORKTREE_TOOL_NAME, json!({}))?,
            function_call(original_pwd_call_id, "exec_command", pwd_args)?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "enter a managed worktree from a subdirectory, check pwd, exit it, and check pwd",
        PermissionProfile::Disabled,
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value = serde_json::from_str(&enter_output)
        .with_context(|| format!("enter_worktree output should be JSON: {enter_output}"))?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;
    let expected_cwd = Path::new(worktree_path)
        .join("codex-rs")
        .canonicalize()
        .with_context(|| {
            format!("canonicalize expected worktree subdir under `{worktree_path}`")
        })?;
    assert_eq!(
        enter_output.get("cwd"),
        Some(&json!(expected_cwd.to_string_lossy().to_string()))
    );

    let worktree_pwd = exec_stdout(
        &request_log
            .function_call_output_text(pwd_call_id)
            .context("missing worktree pwd output")?,
    )?;
    let worktree_pwd = Path::new(&worktree_pwd)
        .canonicalize()
        .with_context(|| format!("canonicalize worktree pwd output `{worktree_pwd}`"))?;
    assert_eq!(worktree_pwd, expected_cwd);

    let exit_output = request_log
        .function_call_output_text(exit_call_id)
        .context("missing exit_worktree output")?;
    let exit_output: Value =
        serde_json::from_str(&exit_output).context("exit_worktree output should be JSON")?;
    assert_eq!(
        exit_output.get("cwd"),
        Some(&json!(test.config.cwd.to_string_lossy().to_string()))
    );

    let original_pwd = exec_stdout(
        &request_log
            .function_call_output_text(original_pwd_call_id)
            .context("missing original pwd output")?,
    )?;
    assert_eq!(
        Path::new(&original_pwd)
            .canonicalize()
            .with_context(|| format!("canonicalize original pwd output `{original_pwd}`"))?,
        test.config.cwd.as_path().canonicalize()?
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_existing_worktree_uses_requested_root_from_subdirectory() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let fixture = tempfile::TempDir::new()?;
    let repo = fixture.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_git_repo(&repo)?;
    let subdir = commit_tracked_subdir(&repo, "codex-rs")?;
    let managed =
        codex_git_utils::create_or_reuse_managed_worktree(&repo, "codex-existing-subdir")?;

    let subdir = dunce::canonicalize(subdir)?.abs();
    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(move |config| {
        config.cwd = subdir;
        config.workspace_roots = vec![config.cwd.clone()];
        config
            .features
            .enable(Feature::UnifiedExec)
            .expect("test config should enable unified exec");
    });
    let test = builder.build(&server).await?;

    let enter_call_id = "enter-existing-subdir-worktree";
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "path": managed.path }),
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
        "adopt an existing managed worktree root from a subdirectory and check cwd",
        PermissionProfile::workspace_write(),
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value = serde_json::from_str(&enter_output)
        .with_context(|| format!("enter_worktree output should be JSON: {enter_output}"))?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;
    let expected_cwd = Path::new(worktree_path)
        .canonicalize()
        .with_context(|| format!("canonicalize requested worktree root `{worktree_path}`"))?;
    assert_eq!(
        enter_output.get("cwd"),
        Some(&json!(expected_cwd.to_string_lossy().to_string()))
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_symlinked_directory_uses_canonical_target() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let fixture = tempfile::TempDir::new()?;
    let repo = fixture.path().join("repo");
    let outside = fixture.path().join("outside");
    std::fs::create_dir(&repo)?;
    std::fs::create_dir(&outside)?;
    init_git_repo(&repo)?;
    let subdir = commit_tracked_subdir(&repo, "codex-rs")?;
    let managed = codex_git_utils::create_or_reuse_managed_worktree(&repo, "codex-symlink-subdir")?;
    std::fs::remove_dir_all(managed.path.join("codex-rs"))?;
    symlink_dir(&outside, managed.path.join("codex-rs"))?;

    let subdir = dunce::canonicalize(subdir)?.abs();
    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(move |config| {
        config.cwd = subdir;
        config.workspace_roots = vec![config.cwd.clone()];
    });
    let test = builder.build(&server).await?;

    let enter_call_id = "enter-symlink-subdir-worktree";
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "path": managed.path.join("codex-rs") }),
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
        "enter an existing managed worktree whose matching subdirectory escapes via symlink",
        PermissionProfile::workspace_write(),
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value = serde_json::from_str(&enter_output)
        .with_context(|| format!("enter_worktree output should be JSON: {enter_output}"))?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;
    assert_eq!(enter_output.get("cwd"), Some(&json!(worktree_path)));
    assert_eq!(
        Path::new(worktree_path).canonicalize()?,
        outside.canonicalize()?
    );

    Ok(())
}
