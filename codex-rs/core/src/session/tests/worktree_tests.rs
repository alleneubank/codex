use super::*;
use pretty_assertions::assert_eq;

fn init_worktree_tool_repo(repo_path: &Path) -> anyhow::Result<()> {
    run_worktree_tool_git(repo_path, &["init"])?;
    run_worktree_tool_git(repo_path, &["checkout", "-B", "main"])?;
    run_worktree_tool_git(repo_path, &["config", "core.autocrlf", "false"])?;
    std::fs::write(repo_path.join("README.md"), "hello\n")?;
    run_worktree_tool_git(repo_path, &["add", "README.md"])?;
    run_worktree_tool_git(
        repo_path,
        &[
            "-c",
            "user.name=Tester",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial commit",
        ],
    )?;
    Ok(())
}

fn run_worktree_tool_git(repo_path: &Path, args: &[&str]) -> anyhow::Result<()> {
    let output = Command::new("git")
        .current_dir(repo_path)
        .args(args)
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed in {}: stdout={} stderr={}",
            args,
            repo_path.display(),
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

async fn make_worktree_tool_session(
    repo_path: &Path,
) -> anyhow::Result<(Arc<Session>, Arc<TurnContext>)> {
    let (session, turn_context, _rx) = make_worktree_tool_session_with_rx(repo_path).await?;
    Ok((session, turn_context))
}

async fn make_worktree_tool_session_with_rx(
    repo_path: &Path,
) -> anyhow::Result<(
    Arc<Session>,
    Arc<TurnContext>,
    async_channel::Receiver<Event>,
)> {
    let repo_path = repo_path.abs();
    let (session, rx) = make_session_with_config_and_rx(move |config| {
        config.cwd = repo_path;
    })
    .await?;
    let turn_context = session.new_default_turn().await;
    Ok((session, turn_context, rx))
}

fn worktree_tool_invocation(
    session: Arc<Session>,
    turn_context: Arc<TurnContext>,
    tool_name: &str,
    arguments: serde_json::Value,
) -> ToolInvocation {
    ToolInvocation {
        session,
        step_context: StepContext::for_test(Arc::clone(&turn_context)),
        turn: turn_context,
        cancellation_token: CancellationToken::new(),
        tracker: Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new())),
        call_id: format!("{tool_name}-call"),
        tool_name: codex_tools::ToolName::plain(tool_name),
        source: ToolCallSource::Direct,
        payload: ToolPayload::Function {
            arguments: arguments.to_string(),
        },
    }
}

async fn enter_worktree_result(
    session: Arc<Session>,
    turn_context: Arc<TurnContext>,
    arguments: serde_json::Value,
) -> std::result::Result<(), FunctionCallError> {
    EnterWorktreeHandler
        .handle(worktree_tool_invocation(
            session,
            turn_context,
            "enter_worktree",
            arguments,
        ))
        .await
        .map(|_| ())
}

async fn exit_worktree_result(
    session: Arc<Session>,
    turn_context: Arc<TurnContext>,
) -> std::result::Result<(), FunctionCallError> {
    ExitWorktreeHandler
        .handle(worktree_tool_invocation(
            session,
            turn_context,
            "exit_worktree",
            json!({}),
        ))
        .await
        .map(|_| ())
}

async fn expect_thread_settings_applied(rx: &async_channel::Receiver<Event>) {
    loop {
        let event = timeout(Duration::from_secs(/*secs*/ 5), rx.recv())
            .await
            .expect("timed out waiting for ThreadSettingsApplied")
            .expect("event stream should stay open");
        if matches!(
            event.msg,
            codex_protocol::protocol::EventMsg::ThreadSettingsApplied(_)
        ) {
            return;
        }
    }
}

fn assert_respond_to_model(result: std::result::Result<(), FunctionCallError>, expected: &str) {
    let Err(FunctionCallError::RespondToModel(output)) = result else {
        panic!("expected worktree tool to respond to model with an error");
    };
    assert!(
        output.contains(expected),
        "expected output to contain {expected:?}, got {output:?}"
    );
}

fn response_items_text(items: &[codex_protocol::models::ResponseItem]) -> String {
    let mut text = String::new();
    for item in items {
        let codex_protocol::models::ResponseItem::Message { content, .. } = item else {
            continue;
        };
        for content in content {
            match content {
                codex_protocol::models::ContentItem::InputText { text: item_text }
                | codex_protocol::models::ContentItem::OutputText { text: item_text } => {
                    text.push_str(item_text);
                    text.push('\n');
                }
                codex_protocol::models::ContentItem::InputImage { .. } => {}
                codex_protocol::models::ContentItem::InputAudio { .. } => {}
            }
        }
    }
    text
}

#[tokio::test]
async fn enter_worktree_rejects_name_and_path_together() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, turn_context) = make_worktree_tool_session(&repo).await?;

    let result = enter_worktree_result(
        session,
        turn_context,
        json!({
            "name": "codex-test",
            "path": ".",
        }),
    )
    .await;

    assert_respond_to_model(
        result,
        "enter_worktree accepts either `name` or `path`, not both",
    );
    Ok(())
}

#[tokio::test]
async fn exit_worktree_without_active_worktree_fails_clearly() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, turn_context) = make_worktree_tool_session(&repo).await?;
    let result = exit_worktree_result(session, turn_context).await;

    assert_respond_to_model(result, "no active worktree to exit");
    Ok(())
}

#[tokio::test]
async fn enter_worktree_rejects_managed_creation_without_write_permission() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    Arc::get_mut(&mut turn_context)
        .expect("single turn context ref")
        .permission_profile = PermissionProfile::read_only();

    let result = enter_worktree_result(
        session,
        turn_context,
        json!({
            "name": "codex-read-only",
        }),
    )
    .await;

    assert_respond_to_model(result, "requires filesystem write permission");
    let info = codex_git_utils::inspect_worktree(&repo)?;
    assert!(!codex_git_utils::managed_worktree_path(&info.common_dir, "codex-read-only")?.exists());
    Ok(())
}

#[tokio::test]
async fn enter_worktree_path_rejects_another_repository() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let other_repo = temp.path().join("other-repo");
    std::fs::create_dir(&repo)?;
    std::fs::create_dir(&other_repo)?;
    init_worktree_tool_repo(&repo)?;
    init_worktree_tool_repo(&other_repo)?;
    let (session, turn_context) = make_worktree_tool_session(&repo).await?;

    let result = enter_worktree_result(
        session,
        turn_context,
        json!({
            "path": other_repo,
        }),
    )
    .await;

    assert_respond_to_model(result, "belongs to git common dir");
    Ok(())
}

#[tokio::test]
async fn enter_worktree_path_rejects_unmanaged_same_repo_worktree() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let unmanaged = temp.path().join("unmanaged");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let info = codex_git_utils::inspect_worktree(&repo)?;
    std::fs::create_dir_all(codex_git_utils::managed_worktrees_dir(&info.common_dir))?;
    run_worktree_tool_git(
        &repo,
        &["worktree", "add", unmanaged.to_str().expect("utf-8 path")],
    )?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    Arc::get_mut(&mut turn_context)
        .expect("single turn context ref")
        .permission_profile = PermissionProfile::Disabled;

    let result = enter_worktree_result(
        session,
        turn_context,
        json!({
            "path": unmanaged,
        }),
    )
    .await;

    assert_respond_to_model(result, "escapes managed worktree directory");
    Ok(())
}

#[tokio::test]
async fn enter_worktree_path_accepts_existing_managed_worktree() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let managed = codex_git_utils::create_or_reuse_managed_worktree(&repo, "codex-existing")?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    Arc::get_mut(&mut turn_context)
        .expect("single turn context ref")
        .permission_profile = PermissionProfile::Disabled;

    enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({
            "path": managed.path,
        }),
    )
    .await?;

    let next_turn = session.new_default_turn().await;
    assert_eq!(
        next_turn.config.cwd.as_path().canonicalize()?,
        managed.info.repo_root
    );
    assert_eq!(
        session
            .active_worktree()
            .await
            .expect("active worktree")
            .worktree_path
            .as_path(),
        managed.info.repo_root.as_path()
    );
    Ok(())
}

#[tokio::test]
async fn enter_worktree_rejects_derived_active_managed_worktree() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let managed = codex_git_utils::create_or_reuse_managed_worktree(&repo, "codex-active")?;
    let (session, mut turn_context) = make_worktree_tool_session(&managed.path).await?;
    Arc::get_mut(&mut turn_context)
        .expect("single turn context ref")
        .permission_profile = PermissionProfile::Disabled;

    let result = enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({
            "name": "codex-nested",
        }),
    )
    .await;

    assert_respond_to_model(result, "already in worktree");
    assert!(
        !codex_git_utils::managed_worktree_path(
            &codex_git_utils::inspect_worktree(&repo)?.common_dir,
            "codex-nested",
        )?
        .exists()
    );
    Ok(())
}

#[tokio::test]
async fn enter_worktree_updates_later_step_context_in_same_turn() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    Arc::get_mut(&mut turn_context)
        .expect("single turn context ref")
        .permission_profile = PermissionProfile::Disabled;
    let original_workspace_roots = turn_context.config.workspace_roots.clone();

    enter_worktree_result(
        Arc::clone(&session),
        Arc::clone(&turn_context),
        json!({
            "name": "codex-same-turn",
        }),
    )
    .await?;

    let next_step = session
        .capture_step_context(Arc::clone(&turn_context))
        .await;
    let next_cwd = next_step
        .environments
        .primary()
        .expect("primary environment")
        .cwd()
        .to_abs_path()?;
    let info = codex_git_utils::inspect_worktree(&repo)?;
    let expected_worktree_path =
        codex_git_utils::managed_worktree_path(&info.common_dir, "codex-same-turn")?;
    assert_eq!(next_cwd.as_path(), expected_worktree_path.abs().as_path());
    let worktree_turn = session.new_default_turn().await;
    assert!(
        worktree_turn
            .config
            .workspace_roots
            .contains(&expected_worktree_path.abs())
    );
    assert!(
        worktree_turn
            .config
            .workspace_roots
            .contains(&info.common_dir.abs())
    );

    exit_worktree_result(Arc::clone(&session), worktree_turn).await?;
    let restored_turn = session.new_default_turn().await;
    assert_eq!(
        restored_turn.config.workspace_roots,
        original_workspace_roots
    );
    Ok(())
}

#[tokio::test]
async fn cwd_settings_update_persists_thread_metadata_cwd() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let next_cwd = temp.path().join("next").abs();
    std::fs::create_dir(next_cwd.as_path())?;
    let (mut session, _turn_context) = make_session_and_context().await;
    let store = attach_in_memory_thread_store(&mut session).await;
    let thread_id = session.thread_id();

    session
        .update_settings(SessionSettingsUpdate {
            environments: Some(TurnEnvironmentSelections::new(
                next_cwd.clone(),
                vec![local(next_cwd.clone())],
            )),
            ..Default::default()
        })
        .await?;
    let stored = codex_thread_store::ThreadStore::read_thread(
        store.as_ref(),
        codex_thread_store::ReadThreadParams {
            thread_id,
            include_archived: true,
            include_history: false,
        },
    )
    .await?;
    assert_eq!(stored.cwd, next_cwd.into_path_buf());
    assert!(store.calls().await.update_thread_metadata >= 1);
    Ok(())
}

#[tokio::test]
async fn enter_worktree_updates_same_turn_filesystem_context_roots() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    Arc::get_mut(&mut turn_context)
        .expect("single turn context ref")
        .permission_profile = PermissionProfile::Disabled;

    enter_worktree_result(
        Arc::clone(&session),
        Arc::clone(&turn_context),
        json!({
            "name": "codex-context",
        }),
    )
    .await?;

    let info = codex_git_utils::inspect_worktree(&repo)?;
    let expected_worktree_path =
        codex_git_utils::managed_worktree_path(&info.common_dir, "codex-context")?.abs();
    let common_dir = info.common_dir.abs();
    let next_step = session
        .capture_step_context(Arc::clone(&turn_context))
        .await;
    assert!(
        next_step
            .workspace_roots
            .contains(&PathUri::from_abs_path(&expected_worktree_path))
    );
    assert!(
        next_step
            .workspace_roots
            .contains(&PathUri::from_abs_path(&common_dir))
    );

    let world_state = session.build_world_state_for_step(next_step.as_ref()).await;
    let rendered = world_state
        .render_full()
        .into_iter()
        .map(crate::context::ContextualUserFragment::into_boxed_response_item)
        .collect::<Vec<_>>();
    let rendered_text = response_items_text(&rendered);
    assert!(rendered_text.contains(expected_worktree_path.to_string_lossy().as_ref()));
    assert!(rendered_text.contains(common_dir.to_string_lossy().as_ref()));
    Ok(())
}

#[tokio::test]
async fn worktree_tools_emit_thread_settings_applied_events() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context, rx) = make_worktree_tool_session_with_rx(&repo).await?;
    Arc::get_mut(&mut turn_context)
        .expect("single turn context ref")
        .permission_profile = PermissionProfile::Disabled;

    enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({
            "name": "codex-events",
        }),
    )
    .await?;
    expect_thread_settings_applied(&rx).await;

    let worktree_turn = session.new_default_turn().await;
    exit_worktree_result(Arc::clone(&session), worktree_turn).await?;
    expect_thread_settings_applied(&rx).await;
    Ok(())
}

#[tokio::test]
async fn enter_worktree_without_args_requires_explicit_name_or_path() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    Arc::get_mut(&mut turn_context)
        .expect("single turn context ref")
        .permission_profile = PermissionProfile::Disabled;

    let result = enter_worktree_result(Arc::clone(&session), turn_context, json!({})).await;

    assert_respond_to_model(result, "enter_worktree requires either `name` or `path`");
    assert!(session.active_worktree().await.is_none());
    Ok(())
}

#[tokio::test]
async fn exit_worktree_derived_state_restores_metadata_original_subdirectory() -> anyhow::Result<()>
{
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let subdir = repo.join("subdir");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    std::fs::create_dir(&subdir)?;
    let (session, mut turn_context) = make_worktree_tool_session(&subdir).await?;
    Arc::get_mut(&mut turn_context)
        .expect("single turn context ref")
        .permission_profile = PermissionProfile::Disabled;

    enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({
            "name": "codex-subdir",
        }),
    )
    .await?;
    session.clear_active_worktree().await;
    let worktree_turn = session.new_default_turn().await;

    exit_worktree_result(Arc::clone(&session), worktree_turn).await?;

    let next_turn = session.new_default_turn().await;
    assert_eq!(
        next_turn.config.cwd.as_path().canonicalize()?,
        subdir.canonicalize()?
    );
    Ok(())
}

#[tokio::test]
async fn exit_worktree_derives_managed_cwd_without_rebinding_workspace_roots() -> anyhow::Result<()>
{
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let managed = codex_git_utils::create_or_reuse_managed_worktree(&repo, "codex-restore")?;
    let (session, turn_context) = make_worktree_tool_session(&managed.path).await?;
    let original_workspace_roots = turn_context.config.workspace_roots.clone();

    exit_worktree_result(Arc::clone(&session), turn_context).await?;

    let next_turn = session.new_default_turn().await;
    assert_eq!(
        next_turn.config.cwd.as_path().canonicalize()?,
        repo.canonicalize()?
    );
    assert_eq!(next_turn.config.workspace_roots, original_workspace_roots);
    Ok(())
}

#[tokio::test]
async fn exit_worktree_rejects_stale_active_state_from_original_checkout() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let original_info = codex_git_utils::inspect_worktree(&repo)?;
    let managed = codex_git_utils::create_or_reuse_managed_worktree(&repo, "codex-stale")?;
    let (session, turn_context) = make_worktree_tool_session(&repo).await?;
    session
        .set_active_worktree(ActiveWorktree {
            original_cwd: repo.abs(),
            original_common_dir: original_info.common_dir,
            original_workspace_roots: None,
            worktree_path: managed.path.abs(),
            branch: managed.info.current_branch,
            name: Some(managed.name),
        })
        .await;

    let result = exit_worktree_result(Arc::clone(&session), turn_context).await;

    assert_respond_to_model(result, "not inside active worktree");
    assert!(session.active_worktree().await.is_none());
    Ok(())
}

#[tokio::test]
async fn settings_cwd_update_outside_active_worktree_clears_active_state() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let original_info = codex_git_utils::inspect_worktree(&repo)?;
    let managed = codex_git_utils::create_or_reuse_managed_worktree(&repo, "codex-clear")?;
    let (session, _turn_context) = make_worktree_tool_session(&managed.path).await?;
    session
        .set_active_worktree(ActiveWorktree {
            original_cwd: repo.abs(),
            original_common_dir: original_info.common_dir,
            original_workspace_roots: None,
            worktree_path: managed.path.abs(),
            branch: managed.info.current_branch,
            name: Some(managed.name),
        })
        .await;

    session
        .update_settings(SessionSettingsUpdate {
            environments: Some(TurnEnvironmentSelections::new(repo.abs(), Vec::new())),
            ..Default::default()
        })
        .await?;

    assert!(session.active_worktree().await.is_none());
    Ok(())
}

#[tokio::test]
async fn enter_worktree_rejects_remote_primary_environment() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    let cwd = PathUri::from_abs_path(&repo.abs());
    let remote_environment = Arc::new(Environment::create_for_tests(Some(
        "ws://127.0.0.1:8765".to_string(),
    ))?);
    Arc::get_mut(&mut turn_context)
        .expect("single turn context ref")
        .environments = TurnEnvironmentSnapshot {
        environments: vec![TurnEnvironmentState::Ready(TurnEnvironment::new(
            REMOTE_ENVIRONMENT_ID.to_string(),
            remote_environment,
            cwd.clone(),
            vec![cwd],
            /*shell*/ None,
        ))],
    };

    let result = enter_worktree_result(
        session,
        turn_context,
        json!({
            "name": "codex-remote-primary",
        }),
    )
    .await;

    assert_respond_to_model(result, "requires a local primary environment");
    Ok(())
}

#[tokio::test]
async fn enter_worktree_retargets_only_local_primary_and_preserves_remote_secondary()
-> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    let original_cwd = PathUri::from_abs_path(&repo.abs());
    let local_environment = turn_context
        .environments
        .primary()
        .expect("default local environment")
        .clone();
    let remote_environment = Arc::new(Environment::create_for_tests(Some(
        "ws://127.0.0.1:8765".to_string(),
    ))?);
    {
        let turn_context = Arc::get_mut(&mut turn_context).expect("single turn context ref");
        turn_context.permission_profile = PermissionProfile::Disabled;
        turn_context.environments = TurnEnvironmentSnapshot {
            environments: vec![
                TurnEnvironmentState::Ready(local_environment),
                TurnEnvironmentState::Ready(TurnEnvironment::new(
                    REMOTE_ENVIRONMENT_ID.to_string(),
                    remote_environment,
                    original_cwd.clone(),
                    vec![original_cwd.clone()],
                    /*shell*/ None,
                )),
            ],
        };
    }

    enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({
            "name": "codex-local-primary",
        }),
    )
    .await?;

    let state = session.state.lock().await;
    let selections = state.session_configuration.environment_selections();
    assert_eq!(selections.len(), 2);
    assert_eq!(selections[0].environment_id, LOCAL_ENVIRONMENT_ID);
    let info = codex_git_utils::inspect_worktree(&repo)?;
    let expected_worktree_path =
        codex_git_utils::managed_worktree_path(&info.common_dir, "codex-local-primary")?;
    assert_eq!(
        selections[0].cwd,
        PathUri::from_abs_path(&expected_worktree_path.abs())
    );
    assert_eq!(selections[1].environment_id, REMOTE_ENVIRONMENT_ID);
    assert_eq!(selections[1].cwd, original_cwd);
    Ok(())
}

#[tokio::test]
async fn enter_worktree_retargets_local_primary_and_preserves_starting_secondary()
-> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    let original_cwd = PathUri::from_abs_path(&repo.abs());
    let manager = Arc::new(
        codex_exec_server::EnvironmentManager::create_for_tests_with_local(
            Some("ws://127.0.0.1:9".to_string()),
            ExecServerRuntimePaths::new(
                std::env::current_exe().expect("current exe"),
                /*codex_linux_sandbox_exe*/ None,
            )?,
        )
        .await,
    );
    let local_environment = turn_context
        .environments
        .primary()
        .expect("default local environment")
        .clone();
    let turn_environments = ThreadEnvironments::new(
        manager,
        default_user_shell(),
        ShellSnapshot::disabled(),
        TurnEnvironmentSnapshot {
            environments: vec![TurnEnvironmentState::Ready(local_environment)],
        },
        /*non_blocking_snapshots*/ true,
    );
    turn_environments.update_selections(&[
        TurnEnvironmentSelection {
            environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
            cwd: original_cwd.clone(),
            workspace_roots: vec![original_cwd.clone()],
        },
        TurnEnvironmentSelection {
            environment_id: REMOTE_ENVIRONMENT_ID.to_string(),
            cwd: original_cwd.clone(),
            workspace_roots: vec![original_cwd.clone()],
        },
    ]);
    let snapshot = turn_environments.snapshot().await;
    assert_eq!(snapshot.turn_environments().count(), 1);
    assert_eq!(snapshot.starting().count(), 1);
    {
        let turn_context = Arc::get_mut(&mut turn_context).expect("single turn context ref");
        turn_context.permission_profile = PermissionProfile::Disabled;
        turn_context.environments = snapshot;
    }

    enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({
            "name": "codex-starting-secondary",
        }),
    )
    .await?;

    let state = session.state.lock().await;
    let selections = state.session_configuration.environment_selections();
    assert_eq!(selections.len(), 2);
    assert_eq!(selections[0].environment_id, LOCAL_ENVIRONMENT_ID);
    let info = codex_git_utils::inspect_worktree(&repo)?;
    let expected_worktree_path =
        codex_git_utils::managed_worktree_path(&info.common_dir, "codex-starting-secondary")?;
    assert_eq!(
        selections[0].cwd,
        PathUri::from_abs_path(&expected_worktree_path.abs())
    );
    assert_eq!(selections[1].environment_id, REMOTE_ENVIRONMENT_ID);
    assert_eq!(selections[1].cwd, original_cwd);
    Ok(())
}

#[tokio::test]
async fn enter_worktree_rejects_starting_only_environment_selections() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    let original_cwd = PathUri::from_abs_path(&repo.abs());
    let manager = Arc::new(
        codex_exec_server::EnvironmentManager::create_for_tests_with_local(
            Some("ws://127.0.0.1:9".to_string()),
            ExecServerRuntimePaths::new(
                std::env::current_exe().expect("current exe"),
                /*codex_linux_sandbox_exe*/ None,
            )?,
        )
        .await,
    );
    let turn_environments = ThreadEnvironments::new(
        manager,
        default_user_shell(),
        ShellSnapshot::disabled(),
        TurnEnvironmentSnapshot::default(),
        /*non_blocking_snapshots*/ true,
    );
    turn_environments.update_selections(&[
        TurnEnvironmentSelection {
            environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
            cwd: original_cwd.clone(),
            workspace_roots: vec![original_cwd.clone()],
        },
        TurnEnvironmentSelection {
            environment_id: REMOTE_ENVIRONMENT_ID.to_string(),
            cwd: original_cwd.clone(),
            workspace_roots: vec![original_cwd.clone()],
        },
    ]);
    let snapshot = turn_environments.snapshot().await;
    assert_eq!(snapshot.turn_environments().count(), 0);
    assert_eq!(snapshot.starting().count(), 2);
    {
        let turn_context = Arc::get_mut(&mut turn_context).expect("single turn context ref");
        turn_context.permission_profile = PermissionProfile::Disabled;
        turn_context.environments = snapshot;
    }

    let result = enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({
            "name": "codex-preserve-starting",
        }),
    )
    .await;

    assert_respond_to_model(result, "requires a local primary environment that is ready");
    assert!(
        !codex_git_utils::managed_worktree_path(
            &codex_git_utils::inspect_worktree(&repo)?.common_dir,
            "codex-preserve-starting",
        )?
        .exists()
    );
    Ok(())
}
