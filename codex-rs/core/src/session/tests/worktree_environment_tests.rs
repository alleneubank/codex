use super::*;
use pretty_assertions::assert_eq;

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
            TurnEnvironmentSelection {
                environment_id: REMOTE_ENVIRONMENT_ID.to_string(),
                cwd: cwd.clone(),
                workspace_roots: vec![cwd.clone()],
                config: EnvironmentConfigState::Ready(writable_environment_config()),
            },
            EnvironmentConfigOrigin::Thread,
            remote_environment,
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
        turn_context.environments = TurnEnvironmentSnapshot {
            environments: vec![
                TurnEnvironmentState::Ready(local_environment),
                TurnEnvironmentState::Ready(TurnEnvironment::new(
                    TurnEnvironmentSelection {
                        environment_id: REMOTE_ENVIRONMENT_ID.to_string(),
                        cwd: original_cwd.clone(),
                        workspace_roots: vec![original_cwd.clone()],
                        config: EnvironmentConfigState::Ready(writable_environment_config()),
                    },
                    EnvironmentConfigOrigin::Thread,
                    remote_environment,
                    /*shell*/ None,
                )),
            ],
        };
    }
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());

    enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({
            "name": "codex-local-primary",
        }),
    )
    .await?;

    let selections = session.services.turn_environments.selections();
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
    let environment_config = writable_environment_config();
    let turn_environments = ThreadEnvironments::new(
        manager,
        default_user_shell(),
        environment_config.clone(),
        ShellSnapshot::disabled(),
        TurnEnvironmentSnapshot {
            environments: vec![TurnEnvironmentState::Ready(local_environment)],
        },
        /*non_blocking_snapshots*/ true,
    );
    turn_environments.update_selections(
        &[
            TurnEnvironmentSelection {
                environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
                cwd: original_cwd.clone(),
                workspace_roots: vec![original_cwd.clone()],
                config: EnvironmentConfigState::FromThread,
            },
            TurnEnvironmentSelection {
                environment_id: REMOTE_ENVIRONMENT_ID.to_string(),
                cwd: original_cwd.clone(),
                workspace_roots: vec![original_cwd.clone()],
                config: EnvironmentConfigState::FromThread,
            },
        ],
        &environment_config,
    );
    let snapshot = turn_environments.snapshot().await;
    assert_eq!(snapshot.turn_environments().count(), 1);
    assert_eq!(snapshot.starting().count(), 1);
    {
        let turn_context_config = Arc::get_mut(&mut turn_context).expect("single turn context ref");
        turn_context_config.environments = TurnEnvironmentSnapshot {
            environments: snapshot.environments.iter().rev().cloned().collect(),
        };
    }
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());

    let result = enter_worktree_result(
        Arc::clone(&session),
        Arc::clone(&turn_context),
        json!({
            "name": "codex-starting-primary",
        }),
    )
    .await;
    assert_respond_to_model(result, "requires a local primary environment that is ready");
    let exit_result = exit_worktree_result(Arc::clone(&session), Arc::clone(&turn_context)).await;
    assert_respond_to_model(
        exit_result,
        "requires a local primary environment that is ready",
    );

    {
        let turn_context_config = Arc::get_mut(&mut turn_context).expect("single turn context ref");
        turn_context_config.environments = snapshot;
    }
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());

    enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({
            "name": "codex-starting-secondary",
        }),
    )
    .await?;

    let selections = session.services.turn_environments.selections();
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
    let environment_config = writable_environment_config();
    let turn_environments = ThreadEnvironments::new(
        manager,
        default_user_shell(),
        environment_config.clone(),
        ShellSnapshot::disabled(),
        TurnEnvironmentSnapshot::default(),
        /*non_blocking_snapshots*/ true,
    );
    turn_environments.update_selections(
        &[
            TurnEnvironmentSelection {
                environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
                cwd: original_cwd.clone(),
                workspace_roots: vec![original_cwd.clone()],
                config: EnvironmentConfigState::FromThread,
            },
            TurnEnvironmentSelection {
                environment_id: REMOTE_ENVIRONMENT_ID.to_string(),
                cwd: original_cwd.clone(),
                workspace_roots: vec![original_cwd.clone()],
                config: EnvironmentConfigState::FromThread,
            },
        ],
        &environment_config,
    );
    let snapshot = turn_environments.snapshot().await;
    assert_eq!(snapshot.turn_environments().count(), 0);
    assert_eq!(snapshot.starting().count(), 2);
    {
        let turn_context = Arc::get_mut(&mut turn_context).expect("single turn context ref");
        turn_context.environments = snapshot;
    }
    set_turn_permission_profile(&mut turn_context, PermissionProfile::Disabled);

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
