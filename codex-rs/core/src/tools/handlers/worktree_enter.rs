use super::*;

pub(super) async fn enter_worktree(
    invocation: ToolInvocation,
) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
    let ToolInvocation {
        session,
        turn,
        step_context,
        payload,
        ..
    } = invocation;
    let arguments = match payload {
        ToolPayload::Function { arguments } => arguments,
        _ => {
            return Err(worktree_model_error(format!(
                "{ENTER_WORKTREE_TOOL_NAME} handler received unsupported payload"
            )));
        }
    };
    let primary_environment =
        local_primary_environment(ENTER_WORKTREE_TOOL_NAME, &step_context.environments)?;
    let original_cwd = primary_environment.cwd().to_abs_path().map_err(|err| {
        worktree_model_error(format!(
            "{ENTER_WORKTREE_TOOL_NAME} requires a native local primary environment cwd: {err}"
        ))
    })?;
    let original_workspace_roots = primary_environment
        .workspace_roots()
        .iter()
        .map(|workspace_root| {
            workspace_root.to_abs_path().map_err(|err| {
                worktree_model_error(format!(
                    "{ENTER_WORKTREE_TOOL_NAME} requires native local primary environment workspace roots: {err}"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let file_system_sandbox_policy = primary_environment
        .permission_profile_with_workspace_roots()
        .file_system_sandbox_policy();
    let owner_thread_id = session.thread_id().to_string();
    let _settings_guard = acquire_thread_settings_persistence_lock(&session).await;
    if let Some(active_worktree_state) =
        active_or_derived_worktree(&session, original_cwd.as_path()).await?
    {
        let active_worktree = active_worktree_state.active_worktree();
        return Err(worktree_model_error(format!(
            "already in worktree `{}`; call {EXIT_WORKTREE_TOOL_NAME} before entering another worktree",
            active_worktree.worktree_path.to_string_lossy()
        )));
    }

    let args: EnterWorktreeArgs = parse_arguments(&arguments).map_err(bound_worktree_error)?;
    let entry = match (args.name, args.path) {
        (Some(_), Some(_)) => {
            return Err(worktree_model_error(
                "enter_worktree accepts either `name` or `path`, not both".to_string(),
            ));
        }
        (Some(name), None) => {
            let original_info =
                inspect_worktree_blocking(original_cwd.as_path().to_path_buf()).await?;
            preflight_managed_worktree_output(&original_info, &original_cwd, &name)?;
            ensure_managed_worktree_writes_allowed(
                ENTER_WORKTREE_TOOL_NAME,
                &file_system_sandbox_policy,
                original_cwd.as_path(),
                &original_info,
                &name,
            )?;
            let managed = create_or_reuse_managed_worktree_blocking(
                original_cwd.as_path().to_path_buf(),
                name,
            )
            .await?;
            let worktree_path = absolute_path(managed.path, "managed worktree path")?;
            let worktree_cwd = matching_worktree_cwd(
                original_info.repo_root.as_path(),
                original_cwd.as_path(),
                worktree_path.as_path(),
            )?;
            let ownership = if managed.created {
                ActiveWorktreeOwnership::ManagedByCodex(original_info.common_dir.clone())
            } else {
                let owned_by_session = read_worktree_metadata_blocking(
                    original_info.common_dir.clone(),
                    managed.name.clone(),
                )
                .await?
                .and_then(|metadata| metadata.owner_thread_id)
                .as_deref()
                    == Some(owner_thread_id.as_str());
                if owned_by_session {
                    ActiveWorktreeOwnership::ManagedByCodex(original_info.common_dir.clone())
                } else {
                    ensure_worktree_paths_writable(
                        ENTER_WORKTREE_TOOL_NAME,
                        &file_system_sandbox_policy,
                        original_cwd.as_path(),
                        &[
                            original_cwd.as_path().to_path_buf(),
                            worktree_path.as_path().to_path_buf(),
                        ],
                    )?;
                    ActiveWorktreeOwnership::Adopted
                }
            };
            WorktreeEntry {
                worktree_cwd,
                worktree_path,
                branch: managed.info.current_branch,
                name: Some(managed.name),
                created: Some(managed.created),
                created_branch: managed.created_branch,
                ownership,
            }
        }
        (None, Some(path)) => {
            if path.is_empty() {
                return Err(worktree_model_error(
                    "enter_worktree `path` must not be empty".to_string(),
                ));
            }
            let candidate_path = canonicalize_for_worktree_check(&resolve_candidate_path(
                original_cwd.as_path(),
                &path,
            ))?;
            if !candidate_path.is_dir() {
                return Err(worktree_model_error(format!(
                    "enter_worktree path `{}` must be an existing directory",
                    candidate_path.display()
                )));
            }
            let candidate_info = inspect_optional_worktree_blocking(candidate_path.clone()).await?;
            let same_session_managed_worktree = if let Some(info) = candidate_info.as_ref()
                && let Some(name) = managed_worktree_name(&info.common_dir, &info.repo_root)
                && inspect_optional_worktree_blocking(original_cwd.as_path().to_path_buf())
                    .await?
                    .is_some_and(|original| original.common_dir == info.common_dir)
            {
                read_worktree_metadata_blocking(info.common_dir.clone(), name)
                    .await?
                    .and_then(|metadata| metadata.owner_thread_id)
                    .as_deref()
                    == Some(owner_thread_id.as_str())
            } else {
                false
            };
            let mut required_paths = vec![original_cwd.as_path().to_path_buf()];
            if !same_session_managed_worktree {
                required_paths.push(candidate_path.clone());
            }
            // The same repository's managed area has the internal-git authority used by
            // name-based creation. Other repositories and ordinary directories need an
            // existing destination grant before adoption can retarget workspace roots.
            ensure_worktree_paths_writable(
                ENTER_WORKTREE_TOOL_NAME,
                &file_system_sandbox_policy,
                original_cwd.as_path(),
                &required_paths,
            )?;
            let worktree_cwd = absolute_path(candidate_path.clone(), "adopted workdir cwd")?;
            let (worktree_path, branch, name, ownership) = match candidate_info {
                Some(info) => {
                    let name = managed_worktree_name(&info.common_dir, &info.repo_root);
                    (
                        absolute_path(info.repo_root, "adopted worktree path")?,
                        info.current_branch,
                        name,
                        if same_session_managed_worktree {
                            ActiveWorktreeOwnership::ManagedByCodex(info.common_dir)
                        } else {
                            ActiveWorktreeOwnership::Adopted
                        },
                    )
                }
                None => (
                    absolute_path(candidate_path, "adopted workdir path")?,
                    None,
                    None,
                    ActiveWorktreeOwnership::Adopted,
                ),
            };
            WorktreeEntry {
                worktree_cwd,
                worktree_path,
                branch,
                name,
                created: None,
                created_branch: None,
                ownership,
            }
        }
        (None, None) => {
            return Err(worktree_model_error(
                "enter_worktree requires either `name` or `path`".to_string(),
            ));
        }
    };
    let enter_output = WorktreeOutput {
        cwd: entry.worktree_cwd.to_string_lossy().to_string(),
        worktree_path: entry.worktree_path.to_string_lossy().to_string(),
        original_cwd: original_cwd.to_string_lossy().to_string(),
        branch: entry.branch.clone(),
        name: entry.name.clone(),
        created: entry.created,
    };
    if let Err(err) = ensure_worktree_output_fits_context(&enter_output) {
        if entry.created == Some(true)
            && let Err(cleanup_err) = remove_created_managed_worktree_blocking(
                original_cwd.as_path().to_path_buf(),
                entry.worktree_path.as_path().to_path_buf(),
                entry.created_branch,
            )
            .await
        {
            return Err(worktree_model_error(format!(
                "{err}; failed to roll back the newly created worktree: {cleanup_err}"
            )));
        }
        return Err(err);
    }
    if entry.created == Some(true)
        && let (ActiveWorktreeOwnership::ManagedByCodex(original_common_dir), Some(name)) =
            (&entry.ownership, entry.name.as_deref())
        && let Err(err) = write_worktree_metadata_blocking(
            original_common_dir.clone(),
            name.to_string(),
            original_cwd.clone(),
            owner_thread_id,
        )
        .await
    {
        if let Err(cleanup_err) = remove_created_managed_worktree_blocking(
            original_cwd.as_path().to_path_buf(),
            entry.worktree_path.as_path().to_path_buf(),
            entry.created_branch.clone(),
        )
        .await
        {
            return Err(worktree_model_error(format!(
                "{err}; failed to roll back the newly created worktree: {cleanup_err}"
            )));
        }
        return Err(err);
    }

    let entered_workspace_root = match &entry.ownership {
        ActiveWorktreeOwnership::ManagedByCodex(_) => &entry.worktree_path,
        ActiveWorktreeOwnership::Adopted => &entry.worktree_cwd,
    };
    let updates = cwd_settings_update(
        ENTER_WORKTREE_TOOL_NAME,
        &original_cwd,
        &entry.worktree_cwd,
        &step_context.environments,
        &PrimaryWorkspaceRootsUpdate::Replace(workspace_roots_for_enter(
            &original_workspace_roots,
            &original_cwd,
            &entry.worktree_cwd,
            entered_workspace_root,
        )),
    )?;
    let commit = session
        .update_settings(updates)
        .await
        .map_err(|err| worktree_model_error(format!("failed to enter worktree: {err}")))?;
    session
        .persist_cwd_metadata(entry.worktree_cwd.clone())
        .await;
    session
        .set_active_worktree(ActiveWorktree {
            original_cwd: original_cwd.clone(),
            original_workspace_roots: Some(original_workspace_roots),
            worktree_path: entry.worktree_path,
            branch: entry.branch,
            name: entry.name,
            ownership: entry.ownership,
        })
        .await;
    session
        .send_event(
            &turn,
            thread_settings_applied_event_from_snapshot(&session, commit.snapshot),
        )
        .await;

    output(enter_output)
}
