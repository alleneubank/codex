use super::*;
use pretty_assertions::assert_eq;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_flow_isolates_patch_from_original_checkout() -> Result<()> {
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

    let marker_file = "DOGFOOD_WORKTREE_MARKER.txt";
    let marker_contents = "created inside managed worktree\n";
    let patch =
        format!("*** Begin Patch\n*** Add File: {marker_file}\n+{marker_contents}*** End Patch\n");
    let enter_call_id = "enter-worktree";
    let patch_call_id = "patch-worktree-marker";
    let worktree_status_call_id = "worktree-status";
    let exit_call_id = "exit-worktree";
    let original_status_call_id = "original-status";
    let status_args = json!({
        "cmd": format!("git status --short {marker_file}"),
        "yield_time_ms": 1_000_u64,
        "max_output_tokens": 2_000_u64,
    });
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-dogfood-flow" }),
            )?,
            ev_apply_patch_custom_tool_call(patch_call_id, &patch),
            function_call(worktree_status_call_id, "exec_command", status_args.clone())?,
            function_call(exit_call_id, EXIT_WORKTREE_TOOL_NAME, json!({}))?,
            function_call(original_status_call_id, "exec_command", status_args)?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "enter a worktree, make an isolated patch, exit it, and compare status",
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
    let worktree_marker = Path::new(worktree_path).join(marker_file);
    assert_eq!(
        std::fs::read_to_string(&worktree_marker)
            .with_context(|| format!("read {}", worktree_marker.display()))?,
        marker_contents
    );
    assert!(
        !test.config.cwd.as_path().join(marker_file).exists(),
        "marker file should not be created in the original checkout"
    );

    let worktree_status = exec_stdout(
        &request_log
            .function_call_output_text(worktree_status_call_id)
            .context("missing worktree status output")?,
    )?;
    assert_eq!(worktree_status, format!("?? {marker_file}"));

    let exit_output = request_log
        .function_call_output_text(exit_call_id)
        .context("missing exit_worktree output")?;
    let exit_output: Value =
        serde_json::from_str(&exit_output).context("exit_worktree output should be JSON")?;
    assert_eq!(
        exit_output.get("cwd"),
        Some(&json!(test.config.cwd.to_string_lossy().to_string()))
    );

    let original_status = exec_stdout(
        &request_log
            .function_call_output_text(original_status_call_id)
            .context("missing original status output")?,
    )?;
    assert_eq!(original_status, "");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_worktree_allows_linked_worktree_with_external_common_git_dir() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let (_fixture, linked_worktree) = create_linked_worktree_fixture()?;
    let linked_worktree = dunce::canonicalize(linked_worktree)?.abs();
    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(move |config| {
        config.cwd = linked_worktree;
        config.workspace_roots = vec![config.cwd.clone()];
    });
    let test = builder.build(&server).await?;

    let enter_call_id = "enter-linked-worktree";
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-linked-flow" }),
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
        "enter a managed worktree from a linked worktree",
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
    let worktree_path = Path::new(worktree_path)
        .canonicalize()
        .with_context(|| format!("canonicalize entered worktree path `{worktree_path}`"))?;
    assert!(worktree_path.is_dir());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_worktree_allows_writable_repo_subdirectory_cwd() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let fixture = tempfile::TempDir::new()?;
    let repo = fixture.path().join("repo");
    let subdir = repo.join("codex-rs");
    std::fs::create_dir(&repo)?;
    init_git_repo(&repo)?;
    std::fs::create_dir(&subdir)?;

    let subdir = dunce::canonicalize(subdir)?.abs();
    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(move |config| {
        config.cwd = subdir;
        config.workspace_roots = vec![config.cwd.clone()];
    });
    let test = builder.build(&server).await?;

    let enter_call_id = "enter-subdir-worktree";
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-subdir-flow" }),
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
        "enter a managed worktree from a writable repo subdirectory",
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
    let worktree_path = Path::new(worktree_path)
        .canonicalize()
        .with_context(|| format!("canonicalize entered worktree path `{worktree_path}`"))?;
    assert!(worktree_path.is_dir());

    Ok(())
}
