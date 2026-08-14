use crate::environment_selection::TurnEnvironmentState;
use crate::function_tool::FunctionCallError;
use crate::session::ActiveWorktree;
use crate::session::ActiveWorktreeOwnership;
use crate::session::SessionSettingsUpdate;
use crate::session::thread_settings_applied_event;
use crate::session::turn_context::TurnEnvironment;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::worktree_spec::ENTER_WORKTREE_TOOL_NAME;
use crate::tools::handlers::worktree_spec::EXIT_WORKTREE_TOOL_NAME;
use crate::tools::handlers::worktree_spec::create_enter_worktree_tool;
use crate::tools::handlers::worktree_spec::create_exit_worktree_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_git_utils::ManagedWorktree;
use codex_git_utils::WorktreeInfo;
use codex_git_utils::create_or_reuse_managed_worktree;
use codex_git_utils::inspect_worktree;
use codex_git_utils::managed_worktree_path;
use codex_git_utils::managed_worktrees_dir;
use codex_git_utils::remove_created_managed_worktree;
use codex_git_utils::remove_managed_worktree;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::protocol::TurnEnvironmentSelection;
use codex_protocol::protocol::TurnEnvironmentSelections;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;

#[path = "worktree_helpers.rs"]
mod worktree_helpers;
use worktree_helpers::bound_worktree_error;
use worktree_helpers::canonicalize_for_worktree_check;
use worktree_helpers::ensure_current_cwd_matches_active_worktree;
use worktree_helpers::ensure_worktree_output_fits_context;
use worktree_helpers::git_error;
use worktree_helpers::inspect_optional_worktree_blocking;
use worktree_helpers::output;
use worktree_helpers::read_worktree_metadata_blocking;
use worktree_helpers::worktree_model_error;
use worktree_helpers::write_worktree_metadata_blocking;

const WORKTREE_METADATA_EXTENSION: &str = "codex.json";

/// What worktree tool output is allowed to cost the model's context.
///
/// The cap is deliberately one byte per token. Worktree output is dominated
/// by paths, branch names, and ids, which can tokenize much more densely than
/// prose, so the byte bound must remain conservative without depending on a
/// non-token-accurate average.
const WORKTREE_OUTPUT_MAX_TOKENS: usize = 512;
const WORKTREE_OUTPUT_MAX_BYTES: usize = WORKTREE_OUTPUT_MAX_TOKENS;

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnterWorktreeArgs {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitWorktreeArgs {
    #[serde(default = "default_keep_worktree")]
    keep: bool,
}

fn default_keep_worktree() -> bool {
    true
}

#[derive(Serialize)]
struct WorktreeOutput {
    cwd: String,
    worktree_path: String,
    original_cwd: String,
    branch: Option<String>,
    name: Option<String>,
    created: Option<bool>,
}

#[derive(Serialize)]
struct ExitWorktreeOutput {
    cwd: String,
    worktree_path: String,
    original_cwd: String,
    branch: Option<String>,
    name: Option<String>,
    created: Option<bool>,
    keep: bool,
    removed: bool,
}

#[derive(Deserialize, Serialize)]
struct WorktreeMetadata {
    original_cwd: String,
}

struct WorktreeEntry {
    worktree_cwd: AbsolutePathBuf,
    worktree_path: AbsolutePathBuf,
    branch: Option<String>,
    name: Option<String>,
    created: Option<bool>,
    created_branch: Option<String>,
    ownership: ActiveWorktreeOwnership,
}

enum ActiveWorktreeState {
    Session(ActiveWorktree),
    Derived(ActiveWorktree),
}

impl ActiveWorktreeState {
    fn active_worktree(&self) -> &ActiveWorktree {
        match self {
            Self::Session(active_worktree) | Self::Derived(active_worktree) => active_worktree,
        }
    }
}

pub(crate) struct EnterWorktreeHandler;

impl ToolExecutor<ToolInvocation> for EnterWorktreeHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(ENTER_WORKTREE_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_enter_worktree_tool()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        false
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(enter_worktree(invocation))
    }
}

impl CoreToolRuntime for EnterWorktreeHandler {}

pub(crate) struct ExitWorktreeHandler;

impl ToolExecutor<ToolInvocation> for ExitWorktreeHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(EXIT_WORKTREE_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_exit_worktree_tool()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        false
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(exit_worktree(invocation))
    }
}

impl CoreToolRuntime for ExitWorktreeHandler {}

async fn enter_worktree(
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
                file_system_sandbox_policy,
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
            WorktreeEntry {
                worktree_cwd,
                worktree_path,
                branch: managed.info.current_branch,
                name: Some(managed.name),
                created: Some(managed.created),
                created_branch: managed.created_branch,
                ownership: ActiveWorktreeOwnership::ManagedByCodex(original_info.common_dir),
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
            ensure_worktree_paths_writable(
                ENTER_WORKTREE_TOOL_NAME,
                file_system_sandbox_policy,
                original_cwd.as_path(),
                &[original_cwd.as_path().to_path_buf()],
            )?;
            let candidate_info = inspect_optional_worktree_blocking(candidate_path.clone()).await?;
            let worktree_cwd = absolute_path(candidate_path.clone(), "adopted workdir cwd")?;
            let (worktree_path, branch, name) = match candidate_info {
                Some(info) => {
                    let name = managed_worktree_name(&info.common_dir, &info.repo_root);
                    (
                        absolute_path(info.repo_root, "adopted worktree path")?,
                        info.current_branch,
                        name,
                    )
                }
                None => (
                    absolute_path(candidate_path, "adopted workdir path")?,
                    None,
                    None,
                ),
            };
            WorktreeEntry {
                worktree_cwd,
                worktree_path,
                branch,
                name,
                created: None,
                created_branch: None,
                ownership: ActiveWorktreeOwnership::Adopted,
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
    if let (ActiveWorktreeOwnership::ManagedByCodex(original_common_dir), Some(name)) =
        (&entry.ownership, entry.name.as_deref())
    {
        write_worktree_metadata_blocking(
            original_common_dir.clone(),
            name.to_string(),
            original_cwd.clone(),
        )
        .await?;
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
        PrimaryWorkspaceRootsUpdate::Replace(workspace_roots_for_enter(
            &original_workspace_roots,
            &original_cwd,
            &entry.worktree_cwd,
            entered_workspace_root,
        )),
    )?;
    session
        .update_settings(updates)
        .await
        .map_err(|err| worktree_model_error(format!("failed to enter worktree: {err}")))?;
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
    session.record_worktree_transition();
    session
        .send_event(&turn, thread_settings_applied_event(&session).await)
        .await;

    output(enter_output)
}

async fn exit_worktree(
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
        workspace_roots_update,
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

    session
        .update_settings(updates)
        .await
        .map_err(|err| worktree_model_error(format!("failed to exit worktree: {err}")))?;
    session.clear_active_worktree().await;
    session.record_worktree_transition();
    session
        .send_event(&turn, thread_settings_applied_event(&session).await)
        .await;

    output(exit_output)
}

fn local_primary_environment<'a>(
    tool_name: &str,
    environments: &'a crate::environment_selection::TurnEnvironmentSnapshot,
) -> Result<&'a TurnEnvironment, FunctionCallError> {
    let Some(TurnEnvironmentState::Ready(primary)) = environments.environments.first() else {
        return Err(worktree_model_error(format!(
            "{tool_name} requires a local primary environment that is ready"
        )));
    };
    if primary.environment.is_remote() {
        return Err(worktree_model_error(format!(
            "{tool_name} requires a local primary environment"
        )));
    }
    Ok(primary)
}

fn cwd_settings_update(
    tool_name: &str,
    current_cwd: &AbsolutePathBuf,
    next_cwd: &AbsolutePathBuf,
    environments: &crate::environment_selection::TurnEnvironmentSnapshot,
    workspace_roots_update: PrimaryWorkspaceRootsUpdate,
) -> Result<SessionSettingsUpdate, FunctionCallError> {
    let mut selections = retarget_environment_cwds(tool_name, current_cwd, next_cwd, environments)?;
    if let PrimaryWorkspaceRootsUpdate::Replace(workspace_roots) = workspace_roots_update {
        let Some(primary) = selections.first_mut() else {
            return Err(worktree_model_error(format!(
                "{tool_name} requires a primary environment selection"
            )));
        };
        primary.workspace_roots = workspace_roots.iter().map(PathUri::from_abs_path).collect();
    }
    Ok(SessionSettingsUpdate {
        environments: Some(TurnEnvironmentSelections::new(next_cwd.clone(), selections)),
        ..Default::default()
    })
}

enum PrimaryWorkspaceRootsUpdate {
    Preserve,
    Replace(Vec<AbsolutePathBuf>),
}

fn retarget_environment_cwds(
    tool_name: &str,
    current_cwd: &AbsolutePathBuf,
    next_cwd: &AbsolutePathBuf,
    environments: &crate::environment_selection::TurnEnvironmentSnapshot,
) -> Result<Vec<TurnEnvironmentSelection>, FunctionCallError> {
    let current_cwd = PathUri::from_abs_path(current_cwd);
    let next_cwd = PathUri::from_abs_path(next_cwd);
    let mut selections = environment_selections_preserving_starting(environments);
    let Some(primary) = environments.primary() else {
        return Err(worktree_model_error(format!(
            "{tool_name} requires a local primary environment that is ready"
        )));
    };
    let mut retargeted_primary = false;
    for (index, environment) in selections.iter_mut().enumerate() {
        if index == 0
            && environment.environment_id == primary.selection.environment_id
            && environment.cwd == current_cwd
        {
            environment.cwd = next_cwd.clone();
            retargeted_primary = true;
        }
    }
    if !retargeted_primary {
        return Err(worktree_model_error(format!(
            "{tool_name} requires the local primary environment cwd to match the session cwd"
        )));
    }
    Ok(selections)
}

fn environment_selections_preserving_starting(
    environments: &crate::environment_selection::TurnEnvironmentSnapshot,
) -> Vec<TurnEnvironmentSelection> {
    environments.selections_including_starting()
}

fn workspace_roots_for_enter(
    current_workspace_roots: &[AbsolutePathBuf],
    original_cwd: &AbsolutePathBuf,
    worktree_cwd: &AbsolutePathBuf,
    entered_workspace_root: &AbsolutePathBuf,
) -> Vec<AbsolutePathBuf> {
    let mut workspace_roots = Vec::with_capacity(current_workspace_roots.len() + 1);
    for root in current_workspace_roots {
        let root = if root == original_cwd {
            worktree_cwd.clone()
        } else {
            root.clone()
        };
        push_unique_workspace_root(&mut workspace_roots, root);
    }
    push_unique_workspace_root(&mut workspace_roots, entered_workspace_root.clone());
    workspace_roots
}

fn matching_worktree_cwd(
    original_repo_root: &Path,
    original_cwd: &Path,
    worktree_path: &Path,
) -> Result<AbsolutePathBuf, FunctionCallError> {
    let candidate_cwd = anticipated_worktree_cwd(original_repo_root, original_cwd, worktree_path)?;
    let Ok(canonical_candidate_cwd) = candidate_cwd.canonicalize() else {
        return absolute_path(worktree_path.to_path_buf(), "worktree cwd");
    };
    let canonical_worktree_path = canonicalize_for_worktree_check(worktree_path)?;
    if canonical_candidate_cwd.starts_with(&canonical_worktree_path) {
        absolute_path(candidate_cwd.to_path_buf(), "worktree cwd")
    } else {
        absolute_path(worktree_path.to_path_buf(), "worktree cwd")
    }
}

fn anticipated_worktree_cwd(
    original_repo_root: &Path,
    original_cwd: &Path,
    worktree_path: &Path,
) -> Result<AbsolutePathBuf, FunctionCallError> {
    let original_repo_root = canonicalize_for_worktree_check(original_repo_root)?;
    let original_cwd = canonicalize_for_worktree_check(original_cwd)?;
    let relative_cwd = original_cwd
        .strip_prefix(&original_repo_root)
        .map_err(|err| {
            worktree_model_error(format!(
                "failed to resolve original cwd `{}` relative to repository root `{}`: {err}",
                original_cwd.display(),
                original_repo_root.display()
            ))
        })?;
    absolute_path(worktree_path.join(relative_cwd), "worktree cwd")
}

fn push_unique_workspace_root(
    workspace_roots: &mut Vec<AbsolutePathBuf>,
    workspace_root: AbsolutePathBuf,
) {
    if !workspace_roots.contains(&workspace_root) {
        workspace_roots.push(workspace_root);
    }
}

fn resolve_candidate_path(current_cwd: &Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        current_cwd.join(path)
    }
}

fn absolute_path(path: PathBuf, field: &str) -> Result<AbsolutePathBuf, FunctionCallError> {
    AbsolutePathBuf::try_from(path)
        .map_err(|err| FunctionCallError::Fatal(format!("{field} is not absolute: {err}")))
}

fn preflight_managed_worktree_output(
    original_info: &WorktreeInfo,
    original_cwd: &AbsolutePathBuf,
    name: &str,
) -> Result<(), FunctionCallError> {
    let worktree_path = absolute_path(
        managed_worktree_path(&original_info.common_dir, name).map_err(git_error)?,
        "managed worktree path",
    )?;
    let worktree_cwd = anticipated_worktree_cwd(
        original_info.repo_root.as_path(),
        original_cwd.as_path(),
        worktree_path.as_path(),
    )?;
    ensure_worktree_output_fits_context(&WorktreeOutput {
        cwd: worktree_cwd.to_string_lossy().to_string(),
        worktree_path: worktree_path.to_string_lossy().to_string(),
        original_cwd: original_cwd.to_string_lossy().to_string(),
        // A new managed worktree uses `name` as its branch. Using `false` is
        // conservative because it is longer than the usual `true` value.
        branch: Some(name.to_string()),
        name: Some(name.to_string()),
        created: Some(false),
    })
}

fn ensure_managed_worktree_writes_allowed(
    tool_name: &str,
    file_system_sandbox_policy: FileSystemSandboxPolicy,
    cwd: &Path,
    original_info: &WorktreeInfo,
    name: &str,
) -> Result<(), FunctionCallError> {
    managed_worktree_path(&original_info.common_dir, name).map_err(git_error)?;
    // Creating a linked worktree necessarily mutates the repository's git
    // common dir, which may live outside the current checkout or writable
    // subdirectory. Treat that as internal git bookkeeping for a writable
    // session cwd; git-utils validates that the managed target stays under the
    // common dir.
    ensure_worktree_paths_writable(
        tool_name,
        file_system_sandbox_policy,
        cwd,
        &[cwd.to_path_buf()],
    )
}

fn ensure_worktree_paths_writable(
    tool_name: &str,
    file_system_sandbox_policy: FileSystemSandboxPolicy,
    cwd: &Path,
    paths: &[PathBuf],
) -> Result<(), FunctionCallError> {
    for path in paths {
        if !file_system_sandbox_policy.can_write_path_with_cwd(path, cwd) {
            return Err(worktree_model_error(format!(
                "{tool_name} requires filesystem write permission for `{}` before it can change the session workdir; additional permissions or configuration are required",
                path.display()
            )));
        }
    }
    Ok(())
}

async fn inspect_worktree_blocking(path: PathBuf) -> Result<WorktreeInfo, FunctionCallError> {
    git_blocking(move || inspect_worktree(&path)).await
}

async fn create_or_reuse_managed_worktree_blocking(
    repository_path: PathBuf,
    name: String,
) -> Result<ManagedWorktree, FunctionCallError> {
    git_blocking(move || create_or_reuse_managed_worktree(&repository_path, &name)).await
}

async fn remove_managed_worktree_blocking(
    repository_path: PathBuf,
    worktree_path: PathBuf,
) -> Result<(), FunctionCallError> {
    git_blocking(move || remove_managed_worktree(&repository_path, &worktree_path)).await
}

async fn remove_created_managed_worktree_blocking(
    repository_path: PathBuf,
    worktree_path: PathBuf,
    created_branch: Option<String>,
) -> Result<(), FunctionCallError> {
    git_blocking(move || {
        remove_created_managed_worktree(&repository_path, &worktree_path, created_branch.as_deref())
    })
    .await
}

async fn git_blocking<T>(
    operation: impl FnOnce() -> Result<T, codex_git_utils::GitToolingError> + Send + 'static,
) -> Result<T, FunctionCallError>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|err| {
            worktree_model_error(format!(
                "worktree operation failed to join blocking task: {err}"
            ))
        })?
        .map_err(git_error)
}

async fn active_or_derived_worktree(
    session: &crate::session::session::Session,
    current_cwd: &Path,
) -> Result<Option<ActiveWorktreeState>, FunctionCallError> {
    if let Some(active_worktree) = session.active_worktree().await {
        return Ok(Some(ActiveWorktreeState::Session(active_worktree)));
    }

    derive_active_worktree_from_cwd(current_cwd)
        .await
        .map(|active_worktree| active_worktree.map(ActiveWorktreeState::Derived))
}

async fn active_or_derived_worktree_for_exit(
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

    let Some(active_worktree) = derive_active_worktree_from_cwd(current_cwd.as_path()).await?
    else {
        return Err(worktree_model_error(
            "no active worktree to exit".to_string(),
        ));
    };
    Ok((ActiveWorktreeState::Derived(active_worktree), current_cwd))
}

async fn derive_active_worktree_from_cwd(
    current_cwd: &Path,
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
    let original_cwd =
        read_worktree_metadata_blocking(current_info.common_dir.clone(), name.clone()).await?;
    let original_cwd = match original_cwd {
        Some(original_cwd) => {
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
        ownership: ActiveWorktreeOwnership::ManagedByCodex(current_info.common_dir),
    }))
}

fn original_repo_root_from_common_dir(common_dir: &Path) -> Option<PathBuf> {
    if common_dir.file_name().is_some_and(|name| name == ".git") {
        common_dir.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

fn managed_worktree_name(common_dir: &Path, repo_root: &Path) -> Option<String> {
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
