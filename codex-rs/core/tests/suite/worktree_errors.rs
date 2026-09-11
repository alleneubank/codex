use super::*;
use pretty_assertions::assert_eq;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_worktree_without_args_returns_error_and_preserves_cwd() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree requires a local primary environment"
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

    let enter_call_id = "enter-worktree-without-args";
    let pwd_call_id = "pwd-after-rejected-enter";
    let pwd_args = json!({
        "cmd": "pwd",
        "yield_time_ms": 1_000_u64,
        "max_output_tokens": 2_000_u64,
    });
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(enter_call_id, ENTER_WORKTREE_TOOL_NAME, json!({}))?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_response_created("resp-2"),
            function_call(pwd_call_id, "exec_command", pwd_args)?,
            ev_completed("resp-2"),
        ]),
        sse(vec![
            ev_assistant_message("msg-3", "done"),
            ev_completed("resp-3"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "try entering a worktree without args, then check pwd",
        PermissionProfile::Disabled,
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    assert!(
        enter_output.contains("enter_worktree requires either `name` or `path`"),
        "unexpected enter_worktree output: {enter_output}"
    );

    let pwd = exec_stdout(
        &request_log
            .function_call_output_text(pwd_call_id)
            .context("missing pwd output")?,
    )?;
    assert_eq!(
        Path::new(&pwd).canonicalize()?,
        test.config.cwd.as_path().canonicalize()?
    );

    let common_dir = run_git_for_stdout(
        test.config.cwd.as_path(),
        &["rev-parse", "--git-common-dir"],
    )?;
    assert!(
        !test
            .config
            .cwd
            .as_path()
            .join(common_dir)
            .join("codex/worktrees")
            .exists(),
        "rejected enter_worktree call should not create managed worktrees"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_worktree_rejects_unwritable_adopted_path_without_changing_authority() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree requires a local primary environment"
    );

    // Keep this fixture outside the test workspace and outside the OS temp roots. The latter
    // are excluded so the test observes the workspace-root authority change itself.
    let fixture = tempfile::Builder::new()
        .prefix("codex-worktree-authority-")
        .tempdir_in(std::env::current_dir()?)?;
    let outside = fixture.path().join("outside");
    std::fs::create_dir(&outside)?;
    let protected_file = outside.join("must-not-be-created");

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config.workspace_roots = vec![config.cwd.clone()];
        config
            .features
            .enable(Feature::UnifiedExec)
            .expect("test config should enable unified exec");
    });
    let test = builder.build_with_auto_env(&server).await?;
    init_git_repo(test.config.cwd.as_path())?;

    #[cfg(unix)]
    let candidate_argument = {
        let link = test.config.cwd.join("outside-link");
        symlink_dir(&outside, &link)?;
        link
    };
    #[cfg(not(unix))]
    let candidate_argument = outside.clone();

    let enter_call_id = "reject-outside-enter";
    let pwd_call_id = "pwd-after-rejected-enter";
    let write_call_id = "write-after-rejected-enter";
    let authorized_enter_call_id = "enter-authorized-path";
    let authorized = test.config.cwd.join("authorized-adopted");
    std::fs::create_dir(&authorized)?;
    let authorized_write_call_id = "write-authorized-path";
    let exit_call_id = "exit-authorized-path";
    let write_command = |path: &Path| {
        if cfg!(windows) {
            format!(
                "Set-Content -LiteralPath '{}' -Value protected",
                path.to_string_lossy().replace("'", "''")
            )
        } else {
            format!(
                "printf protected > {}",
                shlex::try_quote(path.to_str().expect("fixture path should be UTF-8"))
                    .expect("fixture path should be shell-quotable")
            )
        }
    };
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "path": candidate_argument }),
            )?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_response_created("resp-2"),
            function_call(
                pwd_call_id,
                "exec_command",
                json!({
                    "cmd": "pwd",
                    "yield_time_ms": 1_000_u64,
                    "max_output_tokens": 2_000_u64,
                }),
            )?,
            function_call(
                write_call_id,
                "exec_command",
                json!({
                    "cmd": write_command(&protected_file),
                    "yield_time_ms": 1_000_u64,
                    "max_output_tokens": 2_000_u64,
                }),
            )?,
            ev_completed("resp-2"),
        ]),
        sse(vec![
            ev_response_created("resp-3"),
            function_call(
                authorized_enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "path": authorized }),
            )?,
            ev_completed("resp-3"),
        ]),
        sse(vec![
            ev_response_created("resp-4"),
            function_call(
                authorized_write_call_id,
                "exec_command",
                json!({
                    "cmd": write_command(&authorized.join("authorized-file")),
                    "yield_time_ms": 1_000_u64,
                    "max_output_tokens": 2_000_u64,
                }),
            )?,
            function_call(exit_call_id, EXIT_WORKTREE_TOOL_NAME, json!({}))?,
            ev_completed("resp-4"),
        ]),
        sse(vec![
            ev_response_created("resp-5"),
            ev_assistant_message("msg-5", "done"),
            ev_completed("resp-5"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    let permission_profile = PermissionProfile::workspace_write_with(
        &[],
        NetworkSandboxPolicy::Restricted,
        /*exclude_tmpdir_env_var*/ true,
        /*exclude_slash_tmp*/ true,
    );
    test.submit_turn_with_permission_profile(
        "reject the outside worktree, verify cwd, then enter the authorized path",
        permission_profile,
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing rejected enter_worktree output")?;
    assert!(
        enter_output.contains("filesystem write permission"),
        "outside adopted path should be rejected: {enter_output}"
    );
    let pwd = exec_stdout(
        &request_log
            .function_call_output_text(pwd_call_id)
            .context("missing pwd output")?,
    )?;
    assert_eq!(
        Path::new(&pwd).canonicalize()?,
        test.config.cwd.as_path().canonicalize()?
    );
    let write_output = request_log
        .function_call_output_text(write_call_id)
        .context("missing rejected outside write output")?;
    assert!(
        !protected_file.exists(),
        "rejected enter_worktree must not grant write access to the outside fixture; enter: {enter_output}; write: {write_output}"
    );
    assert!(
        authorized.join("authorized-file").exists(),
        "an explicitly authorized adopted path should remain writable"
    );
    let exit_output = request_log
        .function_call_output_text(exit_call_id)
        .context("missing exit_worktree output")?;
    assert!(exit_output.contains(test.config.cwd.to_string_lossy().as_ref()));

    Ok(())
}
