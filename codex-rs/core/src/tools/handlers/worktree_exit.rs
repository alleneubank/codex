use super::*;

pub(super) async fn exit_worktree(
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
                "{EXIT_WORKTREE_TOOL_NAME} handler received unsupported payload"
            )));
        }
    };
    let args: ExitWorktreeArgs = parse_arguments(&arguments).map_err(bound_worktree_error)?;
    local_primary_environment(EXIT_WORKTREE_TOOL_NAME, &step_context.environments)?;
    let (active_worktree_state, current_cwd) =
        active_or_derived_worktree_for_exit(&session, &step_context.environments).await?;
    let is_session_active_worktree =
        matches!(&active_worktree_state, ActiveWorktreeState::Session(_));
    let active_worktree = active_worktree_state.active_worktree();
    if !args.keep && matches!(&active_worktree.ownership, ActiveWorktreeOwnership::Adopted) {
        return Err(worktree_model_error(format!(
            "cannot remove an adopted workdir `{}`; call {EXIT_WORKTREE_TOOL_NAME} with `keep: true`",
            active_worktree.worktree_path.display()
        )));
    }
    if let Err(err) =
        ensure_current_cwd_matches_active_worktree(active_worktree, current_cwd.as_path()).await
    {
        if is_session_active_worktree {
            session.clear_active_worktree().await;
        }
        return Err(err);
    }
    let _settings_guard = acquire_thread_settings_persistence_lock(&session).await;

    let workspace_roots_update =
        if let Some(original_workspace_roots) = active_worktree.original_workspace_roots.clone() {
            PrimaryWorkspaceRootsUpdate::Replace(original_workspace_roots)
        } else {
            // A cold-resumed session has no in-memory original workspace-root metadata.
            // Preserve current roots so exiting from a managed worktree does not
            // silently rebind write permission to the parent repository.
            PrimaryWorkspaceRootsUpdate::Preserve
        };
    let updates = cwd_settings_update(
        EXIT_WORKTREE_TOOL_NAME,
        &current_cwd,
        &active_worktree.original_cwd,
        &step_context.environments,
        &workspace_roots_update,
    )?;

    let removed = !args.keep;
    let exit_output = ExitWorktreeOutput {
        cwd: active_worktree.original_cwd.to_string_lossy().to_string(),
        worktree_path: active_worktree.worktree_path.to_string_lossy().to_string(),
        original_cwd: active_worktree.original_cwd.to_string_lossy().to_string(),
        branch: active_worktree.branch.clone(),
        name: active_worktree.name.clone(),
        created: None,
        keep: args.keep,
        removed,
    };
    ensure_worktree_output_fits_context(&exit_output)?;

    if removed {
        remove_managed_worktree_blocking(
            active_worktree.original_cwd.as_path().to_path_buf(),
            active_worktree.worktree_path.as_path().to_path_buf(),
        )
        .await?;
    }

    let commit = session
        .update_settings(updates)
        .await
        .map_err(|err| worktree_model_error(format!("failed to exit worktree: {err}")))?;
    session
        .persist_cwd_metadata(active_worktree.original_cwd.clone())
        .await;
    session.clear_active_worktree().await;
    session
        .send_event(
            &turn,
            thread_settings_applied_event_from_snapshot(&session, commit.snapshot),
        )
        .await;

    output(exit_output)
}
