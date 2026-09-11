use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_worktree_retargets_same_turn_spawned_agent_cwd() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree requires a local primary environment"
    );

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::Collab)
            .expect("test config should enable collab");
    });
    let test = builder.build(&server).await?;
    init_git_repo(test.config.cwd.as_path())?;

    let prompt = "enter a worktree and spawn an agent there";
    let enter_call_id = "enter-worktree";
    let spawn_call_id = "spawn-worker";
    let child_prompt = "report your cwd";
    let spawn_args = serde_json::to_string(&json!({
        "message": child_prompt,
    }))?;
    mount_sse_once_match(
        &server,
        move |request: &wiremock::Request| body_contains(request, prompt),
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-spawn" }),
            )?,
            ev_function_call_with_namespace(
                spawn_call_id,
                MULTI_AGENT_V1_NAMESPACE,
                SPAWN_AGENT_TOOL_NAME,
                &spawn_args,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;
    let child_turn = mount_sse_once_match(
        &server,
        move |request: &wiremock::Request| {
            body_contains(request, child_prompt)
                && !body_contains(request, spawn_call_id)
                && !has_function_call_output(request, enter_call_id)
                && !has_function_call_output(request, spawn_call_id)
        },
        sse(vec![
            ev_response_created("resp-child"),
            ev_completed("resp-child"),
        ]),
    )
    .await;
    let parent_followup = mount_sse_once_match(
        &server,
        move |request: &wiremock::Request| has_function_call_output(request, spawn_call_id),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    test.submit_turn_with_permission_profile(prompt, PermissionProfile::Disabled)
        .await?;

    let enter_output = parent_followup
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value =
        serde_json::from_str(&enter_output).context("enter_worktree output should be JSON")?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;

    let child_requests = child_turn.requests();
    assert!(
        child_requests
            .iter()
            .any(|request| request.body_contains_text(worktree_path)),
        "child request should use worktree cwd {worktree_path}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_worktree_retargets_same_turn_v2_spawned_agent_cwd() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree requires a local primary environment"
    );

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::Collab)
            .expect("test config should enable collab");
        config
            .features
            .enable(Feature::MultiAgentV2)
            .expect("test config should enable multi-agent v2");
    });
    let test = builder.build(&server).await?;
    init_git_repo(test.config.cwd.as_path())?;

    let prompt = "enter a worktree and spawn a v2 agent there";
    let enter_call_id = "enter-worktree";
    let spawn_call_id = "spawn-worker-v2";
    let child_prompt = "report your v2 cwd";
    let spawn_args = serde_json::to_string(&json!({
        "message": child_prompt,
        "task_name": "worker",
    }))?;
    mount_sse_once_match(
        &server,
        move |request: &wiremock::Request| body_contains(request, prompt),
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-spawn-v2" }),
            )?,
            ev_function_call_with_namespace(
                spawn_call_id,
                MULTI_AGENT_V2_NAMESPACE,
                SPAWN_AGENT_TOOL_NAME,
                &spawn_args,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;
    let child_turn = mount_sse_once_match(
        &server,
        move |request: &wiremock::Request| {
            body_contains(request, child_prompt)
                && !body_contains(request, spawn_call_id)
                && !has_function_call_output(request, enter_call_id)
                && !has_function_call_output(request, spawn_call_id)
        },
        sse(vec![
            ev_response_created("resp-child"),
            ev_completed("resp-child"),
        ]),
    )
    .await;
    let parent_followup = mount_sse_once_match(
        &server,
        move |request: &wiremock::Request| has_function_call_output(request, spawn_call_id),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    test.submit_turn_with_permission_profile(prompt, PermissionProfile::Disabled)
        .await?;

    let enter_output = parent_followup
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value =
        serde_json::from_str(&enter_output).context("enter_worktree output should be JSON")?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;

    let child_requests = child_turn.requests();
    assert!(
        child_requests
            .iter()
            .any(|request| request.body_contains_text(worktree_path)),
        "v2 child request should use worktree cwd {worktree_path}"
    );

    Ok(())
}
