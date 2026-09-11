use super::*;
use pretty_assertions::assert_eq;

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
    set_turn_permission_profile(&mut turn_context, PermissionProfile::read_only());

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
async fn enter_worktree_path_adopts_another_repository_subdirectory() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let other_repo = temp.path().join("other-repo");
    std::fs::create_dir(&repo)?;
    std::fs::create_dir(&other_repo)?;
    init_worktree_tool_repo(&repo)?;
    init_worktree_tool_repo(&other_repo)?;
    let subdirectory = other_repo.join("nested");
    std::fs::create_dir(&subdirectory)?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());

    let arguments = json!({ "path": &subdirectory });
    enter_worktree_result(Arc::clone(&session), turn_context, arguments).await?;

    let active_worktree = session
        .active_worktree()
        .await
        .expect("adopted repository should be active");
    assert_eq!(
        active_worktree.worktree_path.as_path(),
        other_repo.canonicalize()?
    );
    assert_eq!(active_worktree.branch.as_deref(), Some("main"));
    assert_eq!(active_worktree.name, None);
    let adopted_config = session.new_default_turn().await.config.clone();
    let subdirectory = subdirectory.canonicalize()?.abs();
    assert_eq!(adopted_config.cwd, subdirectory);
    assert_eq!(adopted_config.workspace_roots, vec![subdirectory]);
    Ok(())
}

#[tokio::test]
async fn enter_worktree_path_does_not_load_instructions_from_untrusted_repository()
-> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let trusted_repo = temp.path().join("trusted-repo");
    let untrusted_repo = temp.path().join("untrusted-repo");
    let codex_home = temp.path().join("codex-home");
    std::fs::create_dir(&trusted_repo)?;
    std::fs::create_dir(&untrusted_repo)?;
    std::fs::create_dir(&codex_home)?;
    init_worktree_tool_repo(&trusted_repo)?;
    init_worktree_tool_repo(&untrusted_repo)?;
    let untrusted_agents = untrusted_repo.join("AGENTS.md");
    std::fs::write(&untrusted_agents, "untrusted repository instructions\n")?;
    let untrusted_agents = PathUri::from_host_native_path(&untrusted_agents)?;

    let trusted_key = project_trust_key(&trusted_repo);
    let untrusted_key = project_trust_key(&untrusted_repo);
    let codex_home = codex_home.abs();
    let (session, mut turn_context, _rx) =
        make_worktree_tool_session_with_config(&trusted_repo, move |config| {
            config.codex_home = codex_home;
            set_project_trust_config(
                config,
                [
                    (trusted_key, TrustLevel::Trusted),
                    (untrusted_key, TrustLevel::Untrusted),
                ],
                /*project_root_markers*/ None,
                TrustLevel::Trusted,
            );
        })
        .await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());
    let source_reload_config = load_latest_config_for_session(&session).await;
    assert!(source_reload_config.active_project.is_trusted());

    enter_worktree_result(
        Arc::clone(&session),
        Arc::clone(&turn_context),
        json!({ "path": &untrusted_repo }),
    )
    .await?;

    let next_step = session
        .capture_step_context(Arc::clone(&turn_context), &CancellationToken::new())
        .await?;
    assert!(
        !next_step
            .loaded_agents_md
            .as_deref()
            .into_iter()
            .flat_map(crate::agents_md::LoadedAgentsMd::sources)
            .any(|source| source == untrusted_agents)
    );

    session.refresh_runtime_config(source_reload_config).await;
    let post_stale_reload_step = session
        .capture_step_context(Arc::clone(&turn_context), &CancellationToken::new())
        .await?;
    assert!(
        !post_stale_reload_step
            .loaded_agents_md
            .as_deref()
            .into_iter()
            .flat_map(crate::agents_md::LoadedAgentsMd::sources)
            .any(|source| source == untrusted_agents)
    );

    let reloaded_config = load_latest_config_for_session(&session).await;
    assert!(reloaded_config.active_project.is_untrusted());
    session.refresh_runtime_config(reloaded_config).await;
    let post_reload_step = session
        .capture_step_context(turn_context, &CancellationToken::new())
        .await?;
    assert!(
        !post_reload_step
            .loaded_agents_md
            .as_deref()
            .into_iter()
            .flat_map(crate::agents_md::LoadedAgentsMd::sources)
            .any(|source| source == untrusted_agents)
    );
    Ok(())
}

#[tokio::test]
async fn enter_worktree_path_honors_untrusted_nested_project_root() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let trusted_repo = temp.path().join("trusted-repo");
    let target_repo = temp.path().join("target-repo");
    let nested_project = target_repo.join("nested-project");
    let nested_cwd = nested_project.join("work");
    let codex_home = temp.path().join("codex-home");
    std::fs::create_dir(&trusted_repo)?;
    std::fs::create_dir(&target_repo)?;
    std::fs::create_dir_all(&nested_cwd)?;
    std::fs::create_dir(&codex_home)?;
    init_worktree_tool_repo(&trusted_repo)?;
    init_worktree_tool_repo(&target_repo)?;
    std::fs::write(nested_project.join(".project-root"), "")?;
    let untrusted_agents = nested_project.join("AGENTS.md");
    std::fs::write(&untrusted_agents, "untrusted nested project instructions\n")?;
    let untrusted_agents = PathUri::from_host_native_path(&untrusted_agents)?;

    let trusted_key = project_trust_key(&trusted_repo);
    let target_key = project_trust_key(&target_repo);
    let nested_key = project_trust_key(&nested_project);
    let nested_cwd_key = project_trust_key(&nested_cwd);
    let codex_home = codex_home.abs();
    let (session, mut turn_context, _rx) =
        make_worktree_tool_session_with_config(&trusted_repo, move |config| {
            config.codex_home = codex_home;
            set_project_config_entries(
                config,
                [
                    (trusted_key, Some(TrustLevel::Trusted)),
                    (target_key, Some(TrustLevel::Trusted)),
                    (nested_key, Some(TrustLevel::Untrusted)),
                    (nested_cwd_key, None),
                ],
                Some(vec![".project-root".to_string()]),
                TrustLevel::Trusted,
            );
        })
        .await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());

    enter_worktree_result(
        Arc::clone(&session),
        Arc::clone(&turn_context),
        json!({ "path": &nested_cwd }),
    )
    .await?;

    let updated_config = session.get_config().await;
    assert_eq!(updated_config.cwd, nested_cwd.canonicalize()?.abs());
    assert!(updated_config.active_project.is_untrusted());
    let next_step = session
        .capture_step_context(turn_context, &CancellationToken::new())
        .await?;
    assert!(
        !next_step
            .loaded_agents_md
            .as_deref()
            .into_iter()
            .flat_map(crate::agents_md::LoadedAgentsMd::sources)
            .any(|source| source == untrusted_agents)
    );
    Ok(())
}

#[tokio::test]
async fn worktree_settings_resolve_trust_after_runtime_config_refresh() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let trusted_repo = temp.path().join("trusted-repo");
    let target_repo = temp.path().join("target-repo");
    std::fs::create_dir(&trusted_repo)?;
    std::fs::create_dir(&target_repo)?;
    init_worktree_tool_repo(&trusted_repo)?;
    init_worktree_tool_repo(&target_repo)?;
    let trusted_key = project_trust_key(&trusted_repo);
    let target_key = project_trust_key(&target_repo);
    let initial_trusted_key = trusted_key.clone();
    let initial_target_key = target_key.clone();
    let (session, _turn_context, _rx) =
        make_worktree_tool_session_with_config(&trusted_repo, move |config| {
            set_project_trust_config(
                config,
                [
                    (initial_trusted_key, TrustLevel::Trusted),
                    (initial_target_key, TrustLevel::Trusted),
                ],
                /*project_root_markers*/ None,
                TrustLevel::Trusted,
            );
        })
        .await?;
    let target_repo = target_repo.abs();
    let settings_update = SessionSettingsUpdate {
        environments: Some(TurnEnvironmentSelections::new(
            target_repo.clone(),
            vec![local(target_repo.clone())],
        )),
        active_project: Some(ActiveProjectSettingsUpdate::Resolved {
            cwd: target_repo.clone(),
            project_root: target_repo.clone(),
            repo_root: Some(target_repo.clone()),
        }),
        ..Default::default()
    };

    let mut refreshed_config = (*session.get_config().await).clone();
    set_project_trust_config(
        &mut refreshed_config,
        [
            (trusted_key, TrustLevel::Trusted),
            (target_key, TrustLevel::Untrusted),
        ],
        /*project_root_markers*/ None,
        TrustLevel::Trusted,
    );
    session.refresh_runtime_config(refreshed_config).await;

    session.update_settings(settings_update).await?;
    let updated_config = session.get_config().await;
    assert!(updated_config.active_project.is_untrusted());
    assert_eq!(updated_config.cwd, target_repo);
    Ok(())
}

#[tokio::test]
async fn runtime_marker_refresh_recomputes_worktree_project_trust() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let source_repo = temp.path().join("source-repo");
    let target_repo = temp.path().join("target-repo");
    let nested_project = target_repo.join("nested-project");
    let nested_cwd = nested_project.join("work");
    let codex_home = temp.path().join("codex-home");
    std::fs::create_dir(&source_repo)?;
    std::fs::create_dir(&target_repo)?;
    std::fs::create_dir_all(&nested_cwd)?;
    std::fs::create_dir(&codex_home)?;
    init_worktree_tool_repo(&source_repo)?;
    init_worktree_tool_repo(&target_repo)?;
    std::fs::write(nested_project.join(".project-root"), "")?;
    let nested_agents = nested_project.join("AGENTS.md");
    std::fs::write(&nested_agents, "newly untrusted nested instructions\n")?;
    let nested_agents = PathUri::from_host_native_path(&nested_agents)?;

    let source_key = project_trust_key(&source_repo);
    let target_key = project_trust_key(&target_repo);
    let nested_key = project_trust_key(&nested_project);
    let refresh_source_key = source_key.clone();
    let refresh_target_key = target_key.clone();
    let refresh_nested_key = nested_key.clone();
    let codex_home = codex_home.abs();
    let (session, mut turn_context, _rx) =
        make_worktree_tool_session_with_config(&source_repo, move |config| {
            config.codex_home = codex_home;
            set_project_trust_config(
                config,
                [
                    (source_key, TrustLevel::Trusted),
                    (target_key, TrustLevel::Trusted),
                    (nested_key, TrustLevel::Untrusted),
                ],
                /*project_root_markers*/ None,
                TrustLevel::Trusted,
            );
        })
        .await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::Disabled);
    enter_worktree_result(
        Arc::clone(&session),
        Arc::clone(&turn_context),
        json!({ "path": &nested_cwd }),
    )
    .await?;
    assert!(session.get_config().await.active_project.is_trusted());

    let mut refreshed_config = (*session.get_config().await).clone();
    set_project_trust_config(
        &mut refreshed_config,
        [
            (refresh_source_key, TrustLevel::Trusted),
            (refresh_target_key, TrustLevel::Trusted),
            (refresh_nested_key, TrustLevel::Untrusted),
        ],
        Some(vec![".project-root".to_string()]),
        TrustLevel::Trusted,
    );
    session.refresh_runtime_config(refreshed_config).await;

    assert!(session.get_config().await.active_project.is_untrusted());
    let next_step = session
        .capture_step_context(turn_context, &CancellationToken::new())
        .await?;
    assert!(
        !next_step
            .loaded_agents_md
            .as_deref()
            .into_iter()
            .flat_map(crate::agents_md::LoadedAgentsMd::sources)
            .any(|source| source == nested_agents)
    );
    Ok(())
}

#[derive(Clone, Copy)]
enum InitialProjectIdentity {
    Unresolved,
    Resolved,
}

async fn assert_in_flight_runtime_refresh_keeps_worktree_transition_untrusted(
    initial_project_identity: InitialProjectIdentity,
) -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let source_repo = temp.path().join("source-repo");
    let target_repo = temp.path().join("target-repo");
    let codex_home = temp.path().join("codex-home");
    std::fs::create_dir(&source_repo)?;
    std::fs::create_dir(&target_repo)?;
    std::fs::create_dir(&codex_home)?;
    init_worktree_tool_repo(&source_repo)?;
    init_worktree_tool_repo(&target_repo)?;
    let target_agents = target_repo.join("AGENTS.md");
    std::fs::write(&target_agents, "newly untrusted repository instructions\n")?;
    let target_agents = PathUri::from_host_native_path(&target_agents)?;

    let source_key = project_trust_key(&source_repo);
    let target_key = project_trust_key(&target_repo);
    let initial_source_key = source_key.clone();
    let initial_target_key = target_key.clone();
    let codex_home = codex_home.abs();
    let (session, mut turn_context, _rx) =
        make_worktree_tool_session_with_config(&source_repo, move |config| {
            config.codex_home = codex_home;
            set_project_trust_config(
                config,
                [
                    (initial_source_key, TrustLevel::Trusted),
                    (initial_target_key, TrustLevel::Trusted),
                ],
                /*project_root_markers*/ None,
                TrustLevel::Trusted,
            );
        })
        .await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());
    if matches!(initial_project_identity, InitialProjectIdentity::Resolved) {
        let source_repo = source_repo.abs();
        session
            .update_settings(SessionSettingsUpdate {
                environments: Some(TurnEnvironmentSelections::new(
                    source_repo.clone(),
                    vec![local(source_repo)],
                )),
                ..Default::default()
            })
            .await?;
        assert!(session.get_config().await.active_project.is_trusted());
    }

    let mut refreshed_config = (*session.get_config().await).clone();
    set_project_trust_config(
        &mut refreshed_config,
        [
            (source_key, TrustLevel::Trusted),
            (target_key, TrustLevel::Untrusted),
        ],
        /*project_root_markers*/ None,
        TrustLevel::Trusted,
    );

    let started = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let generation = session
        .runtime_config_refresh_generation
        .load(std::sync::atomic::Ordering::Acquire)
        .wrapping_add(1);
    *session.runtime_config_refresh_pause.lock().await = Some(RuntimeConfigRefreshPause {
        generation,
        started: Arc::clone(&started),
        resume: Arc::clone(&resume),
    });
    let refresh = {
        let session = Arc::clone(&session);
        tokio::spawn(async move { session.refresh_runtime_config(refreshed_config).await })
    };
    started.notified().await;

    enter_worktree_result(
        Arc::clone(&session),
        Arc::clone(&turn_context),
        json!({ "path": &target_repo }),
    )
    .await?;
    let transition_was_untrusted = session.get_config().await.active_project.is_untrusted();
    let step = session
        .capture_step_context(turn_context, &CancellationToken::new())
        .await?;

    resume.notify_one();
    refresh.await?;
    assert!(transition_was_untrusted);
    assert!(
        !step
            .loaded_agents_md
            .as_deref()
            .into_iter()
            .flat_map(crate::agents_md::LoadedAgentsMd::sources)
            .any(|source| source == target_agents)
    );
    Ok(())
}

#[tokio::test]
async fn in_flight_runtime_refresh_keeps_initial_worktree_transition_untrusted()
-> anyhow::Result<()> {
    assert_in_flight_runtime_refresh_keeps_worktree_transition_untrusted(
        InitialProjectIdentity::Unresolved,
    )
    .await
}

#[tokio::test]
async fn in_flight_runtime_refresh_keeps_resolved_worktree_transition_untrusted()
-> anyhow::Result<()> {
    assert_in_flight_runtime_refresh_keeps_worktree_transition_untrusted(
        InitialProjectIdentity::Resolved,
    )
    .await
}

#[tokio::test]
async fn older_runtime_refresh_cannot_restore_trust_after_newer_untrusted_refresh()
-> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let source_repo = temp.path().join("source-repo");
    let target_repo = temp.path().join("target-repo");
    let codex_home = temp.path().join("codex-home");
    std::fs::create_dir(&source_repo)?;
    std::fs::create_dir(&target_repo)?;
    std::fs::create_dir(&codex_home)?;
    init_worktree_tool_repo(&source_repo)?;
    init_worktree_tool_repo(&target_repo)?;

    let source_key = project_trust_key(&source_repo);
    let target_key = project_trust_key(&target_repo);
    let initial_source_key = source_key.clone();
    let initial_target_key = target_key.clone();
    let codex_home = codex_home.abs();
    let (session, mut turn_context, _rx) =
        make_worktree_tool_session_with_config(&source_repo, move |config| {
            config.codex_home = codex_home;
            set_project_trust_config(
                config,
                [
                    (initial_source_key, TrustLevel::Trusted),
                    (initial_target_key, TrustLevel::Trusted),
                ],
                /*project_root_markers*/ None,
                TrustLevel::Trusted,
            );
        })
        .await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::Disabled);
    enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({ "path": &target_repo }),
    )
    .await?;
    assert!(session.get_config().await.active_project.is_trusted());

    let mut trusted_refresh = (*session.get_config().await).clone();
    set_project_trust_config(
        &mut trusted_refresh,
        [
            (source_key.clone(), TrustLevel::Trusted),
            (target_key.clone(), TrustLevel::Trusted),
        ],
        /*project_root_markers*/ None,
        TrustLevel::Trusted,
    );
    let mut untrusted_refresh = trusted_refresh.clone();
    set_project_trust_config(
        &mut untrusted_refresh,
        [
            (source_key, TrustLevel::Trusted),
            (target_key, TrustLevel::Untrusted),
        ],
        /*project_root_markers*/ None,
        TrustLevel::Trusted,
    );

    let started = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let generation = session
        .runtime_config_refresh_generation
        .load(std::sync::atomic::Ordering::Acquire)
        .wrapping_add(1);
    *session.runtime_config_refresh_pause.lock().await = Some(RuntimeConfigRefreshPause {
        generation,
        started: Arc::clone(&started),
        resume: Arc::clone(&resume),
    });
    let older_refresh = {
        let session = Arc::clone(&session);
        tokio::spawn(async move { session.refresh_runtime_config(trusted_refresh).await })
    };
    started.notified().await;
    session.refresh_runtime_config(untrusted_refresh).await;
    assert!(session.get_config().await.active_project.is_untrusted());

    resume.notify_one();
    older_refresh.await?;
    assert!(session.get_config().await.active_project.is_untrusted());
    Ok(())
}

#[tokio::test]
async fn remote_first_environment_cwd_update_after_enter_worktree_resolves_local_target_trust()
-> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let source_repo = temp.path().join("source-repo");
    let entered_repo = temp.path().join("entered-repo");
    let untrusted_repo = temp.path().join("untrusted-repo");
    let codex_home = temp.path().join("codex-home");
    for repo in [&source_repo, &entered_repo, &untrusted_repo] {
        std::fs::create_dir(repo)?;
        init_worktree_tool_repo(repo)?;
    }
    let untrusted_nested = untrusted_repo.join("nested");
    std::fs::create_dir(&untrusted_nested)?;
    std::fs::create_dir(&codex_home)?;
    let untrusted_agents = untrusted_repo.join("AGENTS.md");
    std::fs::write(&untrusted_agents, "untrusted repository instructions\n")?;
    let untrusted_agents = PathUri::from_host_native_path(&untrusted_agents)?;

    let source_key = project_trust_key(&source_repo);
    let entered_key = project_trust_key(&entered_repo);
    let untrusted_key = project_trust_key(&untrusted_repo);
    let codex_home = codex_home.abs();
    let (session, mut turn_context, _rx) =
        make_worktree_tool_session_with_config(&source_repo, move |config| {
            config.codex_home = codex_home;
            set_project_trust_config(
                config,
                [
                    (source_key, TrustLevel::Trusted),
                    (entered_key, TrustLevel::Trusted),
                    (untrusted_key, TrustLevel::Untrusted),
                ],
                /*project_root_markers*/ None,
                TrustLevel::Trusted,
            );
        })
        .await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());
    enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({ "path": &entered_repo }),
    )
    .await?;
    assert!(session.get_config().await.active_project.is_trusted());

    let remote_selection = session
        .services
        .turn_environments
        .selections()
        .into_iter()
        .find(|selection| selection.environment_id == REMOTE_ENVIRONMENT_ID)
        .expect("remote environment selection");
    let untrusted_nested = untrusted_nested.abs();
    session
        .update_settings(SessionSettingsUpdate {
            environments: Some(TurnEnvironmentSelections::new(
                untrusted_nested.clone(),
                vec![remote_selection.clone(), local(untrusted_nested.clone())],
            )),
            ..Default::default()
        })
        .await?;

    let updated_config = session.get_config().await;
    assert!(updated_config.active_project.is_untrusted());
    assert_eq!(updated_config.cwd, untrusted_nested);
    let next_step = session
        .capture_step_context(session.new_default_turn().await, &CancellationToken::new())
        .await?;
    assert!(
        !next_step
            .loaded_agents_md
            .as_deref()
            .into_iter()
            .flat_map(crate::agents_md::LoadedAgentsMd::sources)
            .any(|source| source == untrusted_agents)
    );

    session
        .update_settings(SessionSettingsUpdate {
            environments: Some(TurnEnvironmentSelections::new(
                source_repo.abs(),
                vec![remote_selection],
            )),
            ..Default::default()
        })
        .await?;
    assert!(
        session.get_config().await.active_project.is_untrusted(),
        "a local fallback without a matching local environment must fail closed"
    );
    Ok(())
}
