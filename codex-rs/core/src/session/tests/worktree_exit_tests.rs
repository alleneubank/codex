use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn enter_worktree_without_args_requires_explicit_name_or_path() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());

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
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());

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
async fn exit_worktree_keep_false_preserves_session_when_removal_fails() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_worktree_tool_repo(&repo)?;
    let (session, mut turn_context) = make_worktree_tool_session(&repo).await?;
    set_turn_permission_profile(&mut turn_context, PermissionProfile::workspace_write());

    enter_worktree_result(
        Arc::clone(&session),
        Arc::clone(&turn_context),
        json!({"name": "codex-dirty-exit"}),
    )
    .await?;
    let active_worktree = session
        .active_worktree()
        .await
        .expect("worktree should be active");
    std::fs::write(active_worktree.worktree_path.join("dirty.txt"), "dirty\n")?;

    let result = exit_worktree_result_with_arguments(
        Arc::clone(&session),
        session.new_default_turn().await,
        json!({"keep": false}),
    )
    .await;

    assert_respond_to_model(result, "worktree operation failed");
    let current_active_worktree = session
        .active_worktree()
        .await
        .expect("failed removal must preserve the active worktree");
    assert_eq!(current_active_worktree, active_worktree);
    assert!(current_active_worktree.worktree_path.exists());
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
            original_workspace_roots: None,
            worktree_path: managed.path.abs(),
            branch: managed.info.current_branch,
            name: Some(managed.name),
            ownership: ActiveWorktreeOwnership::ManagedByCodex(original_info.common_dir),
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
            original_workspace_roots: None,
            worktree_path: managed.path.abs(),
            branch: managed.info.current_branch,
            name: Some(managed.name),
            ownership: ActiveWorktreeOwnership::ManagedByCodex(original_info.common_dir),
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
