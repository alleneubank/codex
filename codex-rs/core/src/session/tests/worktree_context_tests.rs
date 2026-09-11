use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn enter_worktree_updates_later_step_context_in_same_turn() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());
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
        .capture_step_context(Arc::clone(&turn_context), &CancellationToken::new())
        .await?;
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

    exit_worktree_result(Arc::clone(&session), worktree_turn).await?;
    let restored_turn = session.new_default_turn().await;
    assert_eq!(
        restored_turn.config.workspace_roots,
        original_workspace_roots
    );
    Ok(())
}

#[tokio::test]
async fn enter_worktree_uses_primary_environment_state() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let stale_cwd = temp.path().join("stale-cwd");
    let stale_workspace_root = temp.path().join("stale-workspace-root");
    std::fs::create_dir(&repo)?;
    std::fs::create_dir(&stale_cwd)?;
    std::fs::create_dir(&stale_workspace_root)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());

    let expected_environment = turn_context
        .environments
        .primary()
        .expect("primary environment");
    let expected_cwd = expected_environment.cwd().to_abs_path()?;
    let expected_workspace_roots = expected_environment
        .workspace_roots()
        .iter()
        .map(codex_utils_path_uri::PathUri::to_abs_path)
        .collect::<Result<Vec<_>, _>>()?;
    let turn_context_config = Arc::get_mut(&mut turn_context).expect("single turn context ref");
    let turn_config = Arc::make_mut(&mut turn_context_config.config);
    turn_config.cwd = stale_cwd.abs();
    turn_config.workspace_roots = vec![stale_workspace_root.abs()];
    turn_config
        .permissions
        .set_permission_profile(PermissionProfile::read_only())?;

    enter_worktree_result(
        Arc::clone(&session),
        Arc::clone(&turn_context),
        json!({
            "name": "codex-environment-state",
        }),
    )
    .await?;

    let active_worktree = session
        .active_worktree()
        .await
        .expect("worktree should be active");
    assert_eq!(active_worktree.original_cwd, expected_cwd);
    assert_eq!(
        active_worktree.original_workspace_roots,
        Some(expected_workspace_roots.clone())
    );

    let info = codex_git_utils::inspect_worktree(&repo)?;
    let expected_worktree_path =
        codex_git_utils::managed_worktree_path(&info.common_dir, "codex-environment-state")?.abs();
    let next_step = session
        .capture_step_context(Arc::clone(&turn_context), &CancellationToken::new())
        .await?;
    assert!(
        next_step
            .environments
            .primary()
            .expect("primary environment")
            .workspace_roots()
            .contains(&PathUri::from_abs_path(&expected_worktree_path))
    );
    assert!(
        !next_step
            .environments
            .primary()
            .expect("primary environment")
            .workspace_roots()
            .contains(&PathUri::from_abs_path(&stale_workspace_root.abs(),))
    );

    exit_worktree_result(Arc::clone(&session), session.new_default_turn().await).await?;
    let restored_turn = session.new_default_turn().await;
    assert_eq!(restored_turn.config.cwd, expected_cwd);
    assert_eq!(
        restored_turn.config.workspace_roots,
        expected_workspace_roots
    );
    Ok(())
}

#[tokio::test]
async fn generic_cwd_settings_update_does_not_persist_thread_metadata_cwd() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let next_cwd = temp.path().join("next").abs();
    std::fs::create_dir(next_cwd.as_path())?;
    let (mut session, _turn_context) = make_session_and_context().await;
    let store = attach_in_memory_thread_store(&mut session).await;

    session
        .update_settings(SessionSettingsUpdate {
            environments: Some(TurnEnvironmentSelections::new(
                next_cwd.clone(),
                vec![local(next_cwd.clone())],
            )),
            ..Default::default()
        })
        .await?;
    assert_eq!(store.calls().await.update_thread_metadata, 0);
    Ok(())
}

#[tokio::test]
async fn worktree_transitions_persist_thread_metadata_cwd() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let repo = repo.abs();
    let (mut session, _turn_context) = make_session_and_context().await;
    let store = attach_in_memory_thread_store(&mut session).await;
    let thread_id = session.thread_id();
    session
        .update_settings(SessionSettingsUpdate {
            environments: Some(TurnEnvironmentSelections::new(
                repo.clone(),
                vec![local(repo.clone())],
            )),
            ..Default::default()
        })
        .await?;
    let session = Arc::new(session);
    let mut turn_context = session.new_default_turn().await;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());

    enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({
            "name": "codex-metadata",
        }),
    )
    .await?;
    let entered_cwd = session.new_default_turn().await.config.cwd.clone();
    let stored = codex_thread_store::ThreadStore::read_thread(
        store.as_ref(),
        codex_thread_store::ReadThreadParams {
            thread_id,
            include_archived: true,
            include_history: false,
        },
    )
    .await?;
    assert_eq!(stored.cwd, entered_cwd.into_path_buf());

    exit_worktree_result(Arc::clone(&session), session.new_default_turn().await).await?;
    let stored = codex_thread_store::ThreadStore::read_thread(
        store.as_ref(),
        codex_thread_store::ReadThreadParams {
            thread_id,
            include_archived: true,
            include_history: false,
        },
    )
    .await?;
    assert_eq!(stored.cwd, repo.into_path_buf());
    Ok(())
}

#[tokio::test]
async fn enter_worktree_updates_same_turn_filesystem_context_roots() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());

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
    let next_step = session
        .capture_step_context(Arc::clone(&turn_context), &CancellationToken::new())
        .await?;
    assert!(
        next_step
            .environments
            .primary()
            .expect("primary environment")
            .workspace_roots()
            .contains(&PathUri::from_abs_path(&expected_worktree_path))
    );
    let world_state = session
        .build_world_state_for_step(next_step.as_ref())
        .await?;
    let rendered = world_state
        .render_full()
        .into_iter()
        .map(crate::context::ContextualUserFragment::into_boxed_response_item)
        .collect::<Vec<_>>();
    let rendered_text = response_items_text(&rendered);
    assert!(rendered_text.contains(expected_worktree_path.to_string_lossy().as_ref()));
    Ok(())
}

#[cfg_attr(windows, ignore)]
#[tokio::test]
async fn enter_worktree_refreshes_same_turn_repository_skills() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let source_repo = temp.path().join("source-repo");
    let target_repo = temp.path().join("target-repo");
    let codex_home = temp.path().join("codex-home");
    for repo in [&source_repo, &target_repo] {
        std::fs::create_dir(repo)?;
        init_worktree_tool_repo(repo)?;
    }
    std::fs::create_dir(&codex_home)?;
    for (repo, directory, name) in [
        (&source_repo, "source", "source-skill"),
        (&target_repo, "target", "target-skill"),
    ] {
        let skill_dir = repo.join(".agents/skills").join(directory);
        std::fs::create_dir_all(&skill_dir)?;
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name}\n---\n\n# Body\n"),
        )?;
    }
    let source_key = project_trust_key(&source_repo);
    let target_key = project_trust_key(&target_repo);
    let codex_home = codex_home.abs();
    let (session, mut turn_context, _rx) =
        make_worktree_tool_session_with_config(&source_repo, move |config| {
            config.codex_home = codex_home;
            set_project_trust_config(
                config,
                [
                    (source_key, TrustLevel::Trusted),
                    (target_key, TrustLevel::Trusted),
                ],
                /*project_root_markers*/ None,
                TrustLevel::Trusted,
            );
        })
        .await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::Disabled);
    assert!(
        turn_context
            .skills_snapshot()
            .outcome()
            .skills
            .iter()
            .any(|skill| skill.name == "source-skill")
    );

    enter_worktree_result(
        Arc::clone(&session),
        Arc::clone(&turn_context),
        json!({ "path": &target_repo }),
    )
    .await?;
    session
        .capture_step_context(Arc::clone(&turn_context), &CancellationToken::new())
        .await?;

    let skills_snapshot = turn_context.skills_snapshot();
    assert!(
        skills_snapshot
            .outcome()
            .skills
            .iter()
            .any(|skill| skill.name == "target-skill")
    );
    assert!(
        !skills_snapshot
            .outcome()
            .skills
            .iter()
            .any(|skill| skill.name == "source-skill")
    );
    Ok(())
}

#[tokio::test]
async fn worktree_tools_emit_thread_settings_applied_events() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context, rx) = make_worktree_tool_session_with_rx(&repo).await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());

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
async fn worktree_transition_waits_for_thread_settings_persistence_lock() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context, rx) = make_worktree_tool_session_with_rx(&repo).await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());
    while rx.try_recv().is_ok() {}
    let settings_guard = acquire_thread_settings_persistence_lock(&session).await;
    let mut enter = Box::pin(tokio::task::unconstrained(enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({ "name": "codex-serialized" }),
    )));
    assert!(futures::poll!(enter.as_mut()).is_pending());

    let probe_session = Arc::clone(&session);
    let (probe_acquired_tx, mut probe_acquired_rx) = tokio::sync::oneshot::channel();
    let (release_probe_tx, release_probe_rx) = tokio::sync::oneshot::channel();
    let mut release_probe_tx = Some(release_probe_tx);
    let probe_task = tokio::spawn(async move {
        let _guard = acquire_thread_settings_persistence_lock(&probe_session).await;
        let _ = probe_acquired_tx.send(());
        let _ = release_probe_rx.await;
    });
    drop(settings_guard);

    tokio::select! {
        result = &mut enter => {
            result?;
        }
        _ = &mut probe_acquired_rx => {
            let _ = release_probe_tx.take().expect("probe release sender").send(());
            probe_task.await?;
            enter.await?;
            panic!("worktree transition did not reserve settings persistence before mutation");
        }
    };
    let _ = release_probe_tx
        .take()
        .expect("probe release sender")
        .send(());
    probe_task.await?;
    expect_thread_settings_applied(&rx).await;
    Ok(())
}
