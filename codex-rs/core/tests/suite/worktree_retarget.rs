use super::*;
use pretty_assertions::assert_eq;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_tools_retarget_later_cwd_sensitive_tools() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let adopted_fixture = tempfile::tempdir()?;
    let adopted = adopted_fixture.path().join("external-workdir");
    std::fs::create_dir(&adopted)?;
    let adopted = dunce::canonicalize(adopted)?;
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
    let worktree_pwd_call_id = "repo-root-in-worktree";
    let exit_call_id = "exit-worktree";
    let original_pwd_call_id = "repo-root-after-exit";
    let exec_args = json!({
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
                json!({ "path": &adopted }),
            )?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_response_created("resp-2"),
            function_call(worktree_pwd_call_id, "exec_command", exec_args.clone())?,
            function_call(exit_call_id, EXIT_WORKTREE_TOOL_NAME, json!({}))?,
            function_call(original_pwd_call_id, "exec_command", exec_args)?,
            ev_completed("resp-2"),
        ]),
        sse(vec![
            ev_assistant_message("msg-3", "done"),
            ev_completed("resp-3"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "enter a worktree, check pwd, exit it, and check pwd",
        PermissionProfile::Disabled,
    )
    .await?;

    let requests = request_log.requests();
    let advertised_tools = tool_names(&requests[0].body_json());
    assert!(
        advertised_tools.contains(&ENTER_WORKTREE_TOOL_NAME.to_string()),
        "{ENTER_WORKTREE_TOOL_NAME} should be advertised; got {advertised_tools:?}"
    );
    assert!(
        advertised_tools.contains(&EXIT_WORKTREE_TOOL_NAME.to_string()),
        "{EXIT_WORKTREE_TOOL_NAME} should be advertised; got {advertised_tools:?}"
    );
    assert!(
        requests[1].body_contains_text(adopted.to_string_lossy().as_ref()),
        "the later model request should observe the adopted cwd"
    );

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
    assert_eq!(Path::new(worktree_path).canonicalize()?, adopted);

    let worktree_pwd = exec_stdout(
        &request_log
            .function_call_output_text(worktree_pwd_call_id)
            .context("missing worktree pwd output")?,
    )?;
    assert_eq!(
        Path::new(&worktree_pwd).canonicalize()?,
        Path::new(worktree_path).canonicalize()?
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
    assert!(
        Path::new(worktree_path).exists(),
        "default exit_worktree should keep the adopted workdir"
    );

    let original_pwd = exec_stdout(
        &request_log
            .function_call_output_text(original_pwd_call_id)
            .context("missing original pwd output")?,
    )?;
    assert_eq!(
        Path::new(&original_pwd).canonicalize()?,
        test.config.cwd.as_path().canonicalize()?
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_tools_pass_effective_cwd_to_later_hooks() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and hooks require a local primary environment"
    );

    let server = start_mock_server().await;
    let mut builder = test_codex()
        .with_pre_build_hook(|home| {
            let script_path = home.join("worktree_pre_tool_use_hook.py");
            let log_path = home.join("worktree_pre_tool_use_hook_log.jsonl");
            std::fs::write(
                &script_path,
                format!(
                    "import json\nimport sys\nfrom pathlib import Path\npayload = json.load(sys.stdin)\nwith Path(r\"{}\").open(\"a\", encoding=\"utf-8\") as handle:\n    handle.write(json.dumps(payload) + \"\\n\")\n",
                    log_path.display()
                ),
            )
            .expect("write worktree pre-tool hook script");
            std::fs::write(
                home.join("hooks.json"),
                serde_json::json!({
                    "hooks": {
                        "PreToolUse": [{
                            "matcher": "^Bash$",
                            "hooks": [{
                                "type": "command",
                                "command": format!("python3 {}", script_path.display())
                            }]
                        }],
                        "Stop": [{
                            "hooks": [{
                                "type": "command",
                                "command": format!("python3 {}", script_path.display())
                            }]
                        }]
                    }
                })
                .to_string(),
            )
            .expect("write worktree pre-tool hooks configuration");
        })
        .with_config(|config| {
            config
                .features
                .enable(Feature::UnifiedExec)
                .expect("test config should enable unified exec");
            trust_discovered_hooks(config);
        });
    let test = builder.build(&server).await?;
    init_git_repo(test.config.cwd.as_path())?;

    let enter_call_id = "enter-worktree-for-hook";
    let pwd_call_id = "pwd-after-enter-for-hook";
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                function_call(
                    enter_call_id,
                    ENTER_WORKTREE_TOOL_NAME,
                    json!({ "name": "codex-hook-cwd" }),
                )?,
                function_call(
                    pwd_call_id,
                    "exec_command",
                    json!({
                        "cmd": "pwd",
                        "yield_time_ms": 1_000_u64,
                        "max_output_tokens": 2_000_u64,
                    }),
                )?,
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_assistant_message("msg-2", "done"),
                ev_completed("resp-2"),
            ]),
        ],
    )
    .await;

    test.submit_turn_with_permission_profile(
        "enter a worktree, then run a hooked shell command",
        PermissionProfile::Disabled,
    )
    .await?;

    let enter_output = responses
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value = serde_json::from_str(&enter_output)?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?
        .to_string();
    assert_eq!(
        Path::new(&exec_stdout(
            &responses
                .function_call_output_text(pwd_call_id)
                .context("missing pwd output")?,
        )?)
        .canonicalize()?,
        Path::new(&worktree_path).canonicalize()?
    );

    let hook_log = test
        .codex_home_path()
        .join("worktree_pre_tool_use_hook_log.jsonl");
    let hook_inputs: Vec<Value> = std::fs::read_to_string(hook_log)?
        .lines()
        .map(serde_json::from_str)
        .collect::<std::result::Result<_, _>>()?;
    let hook_input = hook_inputs
        .iter()
        .find(|input| input["tool_name"] == "Bash")
        .context("missing hooked shell command input")?;
    assert_eq!(
        Path::new(hook_input["cwd"].as_str().context("hook cwd")?).canonicalize()?,
        Path::new(&worktree_path).canonicalize()?
    );
    let stop_input = hook_inputs
        .iter()
        .find(|input| input["hook_event_name"] == "Stop")
        .context("missing Stop hook input")?;
    assert_eq!(
        Path::new(stop_input["cwd"].as_str().context("stop hook cwd")?).canonicalize()?,
        Path::new(&worktree_path).canonicalize()?
    );

    Ok(())
}
