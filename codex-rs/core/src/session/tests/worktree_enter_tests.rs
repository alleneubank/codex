use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn enter_worktree_path_adopts_unmanaged_same_repo_worktree() -> anyhow::Result<()> {
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
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());

    let arguments = json!({ "path": &unmanaged });
    enter_worktree_result(Arc::clone(&session), turn_context, arguments).await?;

    let active_worktree = session
        .active_worktree()
        .await
        .expect("adopted worktree should be active");
    assert_eq!(
        active_worktree.worktree_path.as_path(),
        unmanaged.canonicalize()?
    );
    Ok(())
}

#[tokio::test]
async fn enter_worktree_path_adopts_plain_directory_from_plain_directory() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let original = temp.path().join("original");
    let adopted = temp.path().join("adopted");
    std::fs::create_dir(&original)?;
    std::fs::create_dir(&adopted)?;
    let (session, mut turn_context) = make_worktree_tool_session(&original).await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());

    enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({ "path": &adopted }),
    )
    .await?;

    let active_worktree = session
        .active_worktree()
        .await
        .expect("adopted directory should be active");
    assert_eq!(
        active_worktree.worktree_path.as_path(),
        adopted.canonicalize()?
    );
    assert_eq!(active_worktree.branch, None);
    assert_eq!(active_worktree.name, None);
    assert_eq!(active_worktree.ownership, ActiveWorktreeOwnership::Adopted);
    assert_eq!(
        session.new_default_turn().await.config.cwd.as_path(),
        adopted.canonicalize()?
    );
    Ok(())
}

#[tokio::test]
async fn enter_worktree_path_treats_undecided_destination_as_untrusted() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let trusted_repo = temp.path().join("trusted-repo");
    let destination = temp.path().join("destination");
    let codex_home = temp.path().join("codex-home");
    std::fs::create_dir(&trusted_repo)?;
    std::fs::create_dir(&destination)?;
    std::fs::create_dir(&codex_home)?;
    init_worktree_tool_repo(&trusted_repo)?;
    let destination_agents = destination.join("AGENTS.md");
    std::fs::write(
        &destination_agents,
        "unconfigured destination instructions\n",
    )?;
    let destination_agents = PathUri::from_host_native_path(&destination_agents)?;
    let trusted_key = project_trust_key(&trusted_repo);
    let destination_key = project_trust_key(&destination);
    let codex_home = codex_home.abs();
    let (session, mut turn_context, _rx) =
        make_worktree_tool_session_with_config(&trusted_repo, move |config| {
            config.codex_home = codex_home;
            set_project_config_entries(
                config,
                [
                    (trusted_key, Some(TrustLevel::Trusted)),
                    (destination_key, None),
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
        json!({ "path": &destination }),
    )
    .await?;

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
            .any(|source| source == destination_agents)
    );
    Ok(())
}

#[tokio::test]
async fn enter_worktree_path_resolves_existing_relative_directory() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let adopted = temp.path().join("adopted");
    std::fs::create_dir(&repo)?;
    std::fs::create_dir(&adopted)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());

    enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({ "path": "../adopted" }),
    )
    .await?;

    assert_eq!(
        session.new_default_turn().await.config.cwd.as_path(),
        adopted.canonicalize()?
    );
    Ok(())
}

#[tokio::test]
async fn enter_worktree_path_rejects_invalid_target_without_state_change() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let missing = temp.path().join("missing");
    let file = temp.path().join("file.txt");
    std::fs::create_dir(&repo)?;
    std::fs::write(&file, "preserve me\n")?;
    init_worktree_tool_repo(&repo)?;
    let (session, turn_context) = make_worktree_tool_session(&repo).await?;
    let original_config = session.new_default_turn().await.config.clone();

    let missing_result = enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({ "path": &missing }),
    )
    .await;
    assert_respond_to_model(missing_result, "failed to canonicalize worktree path");

    let file_result = enter_worktree_result(
        Arc::clone(&session),
        session.new_default_turn().await,
        json!({ "path": &file }),
    )
    .await;
    assert_respond_to_model(file_result, "must be an existing directory");

    let current_config = session.new_default_turn().await.config.clone();
    assert_eq!(
        (&current_config.cwd, &current_config.workspace_roots),
        (&original_config.cwd, &original_config.workspace_roots)
    );
    assert!(session.active_worktree().await.is_none());
    assert!(!missing.exists());
    assert_eq!(std::fs::read_to_string(file)?, "preserve me\n");
    Ok(())
}

#[tokio::test]
async fn exit_worktree_keep_false_rejects_adopted_directory_without_state_change()
-> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let adopted = temp.path().join("adopted");
    let sentinel = adopted.join("sentinel.txt");
    std::fs::create_dir(&repo)?;
    std::fs::create_dir(&adopted)?;
    std::fs::write(&sentinel, "preserve me\n")?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());
    let original_config = session.new_default_turn().await.config.clone();

    enter_worktree_result(
        Arc::clone(&session),
        turn_context,
        json!({ "path": &adopted }),
    )
    .await?;
    let active_worktree = session
        .active_worktree()
        .await
        .expect("adopted directory should be active");
    let adopted_config = session.new_default_turn().await.config.clone();

    let result = exit_worktree_result_with_arguments(
        Arc::clone(&session),
        session.new_default_turn().await,
        json!({ "keep": false }),
    )
    .await;

    assert_respond_to_model(result, "cannot remove an adopted workdir");
    assert_eq!(session.active_worktree().await, Some(active_worktree));
    let current_config = session.new_default_turn().await.config.clone();
    assert_eq!(current_config.cwd, adopted_config.cwd);
    assert_eq!(
        current_config.workspace_roots,
        adopted_config.workspace_roots
    );
    assert_eq!(std::fs::read_to_string(sentinel)?, "preserve me\n");

    exit_worktree_result(Arc::clone(&session), session.new_default_turn().await).await?;
    let restored_config = session.new_default_turn().await.config.clone();
    assert_eq!(restored_config.cwd, original_config.cwd);
    assert_eq!(
        restored_config.workspace_roots,
        original_config.workspace_roots
    );
    assert!(session.active_worktree().await.is_none());
    assert!(adopted.is_dir());
    Ok(())
}

#[tokio::test]
async fn enter_worktree_path_accepts_granted_existing_managed_worktree() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let managed = codex_git_utils::create_or_reuse_managed_worktree(&repo, "codex-existing")?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    set_turn_permission_profile(
        &mut turn_context,
        PermissionProfile::workspace_write_with(
            &[],
            codex_protocol::permissions::NetworkSandboxPolicy::Restricted,
            /*exclude_tmpdir_env_var*/ true,
            /*exclude_slash_tmp*/ true,
        ),
    );
    let turn_context_config = Arc::get_mut(&mut turn_context).expect("single turn context ref");
    Arc::make_mut(&mut turn_context_config.config)
        .permissions
        .set_permission_profile(PermissionProfile::read_only())?;

    let other_repo = temp.path().join("other-repo");
    std::fs::create_dir(&other_repo)?;
    init_worktree_tool_repo(&other_repo)?;
    let foreign = codex_git_utils::create_or_reuse_managed_worktree(&other_repo, "foreign")?;
    let result = enter_worktree_result(
        Arc::clone(&session),
        Arc::clone(&turn_context),
        json!({ "path": foreign.path }),
    )
    .await;
    assert_respond_to_model(result, "requires filesystem write permission");
    assert!(session.active_worktree().await.is_none());

    let result = enter_worktree_result(
        Arc::clone(&session),
        Arc::clone(&turn_context),
        json!({ "path": managed.path }),
    )
    .await;
    assert_respond_to_model(result, "requires filesystem write permission");
    assert!(session.active_worktree().await.is_none());

    set_turn_permission_profile(
        &mut turn_context,
        PermissionProfile::workspace_write_with(
            &[managed.path.clone().abs()],
            codex_protocol::permissions::NetworkSandboxPolicy::Restricted,
            /*exclude_tmpdir_env_var*/ true,
            /*exclude_slash_tmp*/ true,
        ),
    );
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
    let active_worktree = session.active_worktree().await.expect("active worktree");
    assert_eq!(
        active_worktree.worktree_path.as_path(),
        managed.info.repo_root
    );
    assert_eq!(active_worktree.ownership, ActiveWorktreeOwnership::Adopted);
    Ok(())
}

#[tokio::test]
async fn reused_managed_worktree_is_adopted_by_a_different_session() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (owner_session, mut owner_turn) = make_worktree_tool_session(&repo).await?;
    set_turn_permission_profile(&mut owner_turn, PermissionProfile::workspace_write());

    enter_worktree_result(
        Arc::clone(&owner_session),
        owner_turn,
        json!({ "name": "codex-owned" }),
    )
    .await?;
    let owner_worktree = owner_session
        .active_worktree()
        .await
        .expect("owner active worktree");
    assert!(matches!(
        owner_worktree.ownership,
        ActiveWorktreeOwnership::ManagedByCodex(_)
    ));

    let (other_session, mut other_turn) = make_worktree_tool_session(&repo).await?;
    set_turn_permission_profile(&mut other_turn, PermissionProfile::workspace_write());
    let result = enter_worktree_result(
        Arc::clone(&other_session),
        Arc::clone(&other_turn),
        json!({ "name": "codex-owned" }),
    )
    .await;
    assert_respond_to_model(result, "requires filesystem write permission");
    assert!(other_session.active_worktree().await.is_none());

    set_turn_permission_profile(
        &mut other_turn,
        PermissionProfile::workspace_write_with(
            std::slice::from_ref(&owner_worktree.worktree_path),
            codex_protocol::permissions::NetworkSandboxPolicy::Restricted,
            /*exclude_tmpdir_env_var*/ false,
            /*exclude_slash_tmp*/ false,
        ),
    );
    enter_worktree_result(
        Arc::clone(&other_session),
        other_turn,
        json!({ "name": "codex-owned" }),
    )
    .await?;

    let active_worktree = other_session
        .active_worktree()
        .await
        .expect("reused worktree should be active");
    assert_eq!(active_worktree.ownership, ActiveWorktreeOwnership::Adopted);
    let result = exit_worktree_result_with_arguments(
        Arc::clone(&other_session),
        other_session.new_default_turn().await,
        json!({ "keep": false }),
    )
    .await;
    assert_respond_to_model(result, "cannot remove an adopted workdir");
    assert!(active_worktree.worktree_path.exists());
    Ok(())
}

#[tokio::test]
async fn enter_worktree_path_rejects_environment_read_only_profile() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let managed = codex_git_utils::create_or_reuse_managed_worktree(&repo, "codex-env-read-only")?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());
    let turn_context_config = Arc::get_mut(&mut turn_context).expect("single turn context ref");
    let environment = turn_context_config
        .environments
        .environments
        .iter_mut()
        .find_map(|state| match state {
            TurnEnvironmentState::Ready(environment)
                if environment.selection.environment_id == LOCAL_ENVIRONMENT_ID =>
            {
                Some(environment)
            }
            _ => None,
        })
        .expect("local environment");
    environment.config_mut().permission_profile =
        PermissionProfileSnapshot::legacy(PermissionProfile::read_only());

    let result = enter_worktree_result(
        session,
        turn_context,
        json!({
            "path": managed.path,
        }),
    )
    .await;

    assert_respond_to_model(result, "requires filesystem write permission");
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
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());

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
