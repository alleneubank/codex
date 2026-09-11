use super::*;

pub(super) async fn active_or_derived_worktree(
    session: &crate::session::session::Session,
    current_cwd: &Path,
) -> Result<Option<ActiveWorktreeState>, FunctionCallError> {
    if let Some(active_worktree) = session.active_worktree().await {
        return Ok(Some(ActiveWorktreeState::Session(active_worktree)));
    }

    let owner_thread_id = session.thread_id().to_string();
    derive_active_worktree_from_cwd(current_cwd, &owner_thread_id)
        .await
        .map(|active_worktree| active_worktree.map(ActiveWorktreeState::Derived))
}

pub(super) async fn active_or_derived_worktree_for_exit(
    session: &crate::session::session::Session,
    environments: &crate::environment_selection::TurnEnvironmentSnapshot,
) -> Result<(ActiveWorktreeState, AbsolutePathBuf), FunctionCallError> {
    let Some(primary) = environments.primary() else {
        return Err(worktree_model_error(format!(
            "{EXIT_WORKTREE_TOOL_NAME} requires a local primary environment that is ready"
        )));
    };
    let current_cwd = primary.cwd().to_abs_path().map_err(|err| {
        worktree_model_error(format!(
            "{EXIT_WORKTREE_TOOL_NAME} requires a native local primary environment cwd: {err}"
        ))
    })?;
    if let Some(active_worktree) = session.active_worktree().await {
        return Ok((ActiveWorktreeState::Session(active_worktree), current_cwd));
    }

    let owner_thread_id = session.thread_id().to_string();
    let Some(active_worktree) =
        derive_active_worktree_from_cwd(current_cwd.as_path(), &owner_thread_id).await?
    else {
        return Err(worktree_model_error(
            "no active worktree to exit".to_string(),
        ));
    };
    Ok((ActiveWorktreeState::Derived(active_worktree), current_cwd))
}

async fn derive_active_worktree_from_cwd(
    current_cwd: &Path,
    owner_thread_id: &str,
) -> Result<Option<ActiveWorktree>, FunctionCallError> {
    let Some(current_info) = inspect_optional_worktree_blocking(current_cwd.to_path_buf()).await?
    else {
        return Ok(None);
    };
    let managed_base = match managed_worktrees_dir(&current_info.common_dir).canonicalize() {
        Ok(managed_base) => managed_base,
        Err(_) => return Ok(None),
    };
    if !current_info.repo_root.starts_with(&managed_base) {
        return Ok(None);
    }
    let Some(name) = managed_worktree_name_from_base(&managed_base, &current_info.repo_root) else {
        return Ok(None);
    };
    let Some(original_repo_root) = original_repo_root_from_common_dir(&current_info.common_dir)
    else {
        return Ok(None);
    };
    let metadata =
        read_worktree_metadata_blocking(current_info.common_dir.clone(), name.clone()).await?;
    let ownership = if metadata
        .as_ref()
        .and_then(|metadata| metadata.owner_thread_id.as_deref())
        == Some(owner_thread_id)
    {
        ActiveWorktreeOwnership::ManagedByCodex(current_info.common_dir.clone())
    } else {
        ActiveWorktreeOwnership::Adopted
    };
    let original_cwd = match metadata {
        Some(metadata) => {
            let original_cwd = absolute_path(
                PathBuf::from(metadata.original_cwd),
                "managed worktree metadata original cwd",
            )?;
            let original_info = inspect_worktree_blocking(original_cwd.as_path().to_path_buf())
                .await
                .map_err(|err| {
                    worktree_model_error(format!(
                        "failed to validate managed worktree metadata: {err}"
                    ))
                })?;
            if original_info.common_dir != current_info.common_dir {
                return Err(worktree_model_error(format!(
                    "managed worktree metadata points to git common dir `{}`, expected `{}`",
                    original_info.common_dir.display(),
                    current_info.common_dir.display()
                )));
            }
            original_cwd
        }
        None => absolute_path(original_repo_root, "original repository root")?,
    };
    let worktree_path = absolute_path(current_info.repo_root, "worktree path")?;
    Ok(Some(ActiveWorktree {
        original_cwd,
        original_workspace_roots: None,
        worktree_path,
        branch: current_info.current_branch,
        name: Some(name),
        ownership,
    }))
}

fn original_repo_root_from_common_dir(common_dir: &Path) -> Option<PathBuf> {
    if common_dir.file_name().is_some_and(|name| name == ".git") {
        common_dir.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

pub(super) fn managed_worktree_name(common_dir: &Path, repo_root: &Path) -> Option<String> {
    managed_worktrees_dir(common_dir)
        .canonicalize()
        .ok()
        .and_then(|managed_base| managed_worktree_name_from_base(&managed_base, repo_root))
}

fn managed_worktree_name_from_base(managed_base: &Path, repo_root: &Path) -> Option<String> {
    let relative_worktree_path = repo_root.strip_prefix(managed_base).ok()?;
    let mut components = relative_worktree_path.components();
    let std::path::Component::Normal(name) = components.next()? else {
        return None;
    };
    if components.next().is_some() {
        return None;
    }
    Some(name.to_string_lossy().to_string())
}
