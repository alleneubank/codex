use crate::function_tool::FunctionCallError;
use crate::session::ActiveWorktree;
use crate::session::SessionSettingsUpdate;
use crate::session::thread_settings_applied_event;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
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
use codex_git_utils::remove_managed_worktree;
use codex_git_utils::validate_managed_same_repository_worktree_with_info;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::protocol::TurnEnvironmentSelection;
use codex_protocol::protocol::TurnEnvironmentSelections;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_output_truncation::truncate_text;
use codex_utils_path_uri::PathUri;
use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;

const WORKTREE_METADATA_EXTENSION: &str = "codex.json";
const WORKTREE_OUTPUT_MAX_BYTES: usize = 2_048;

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
    ensure_local_primary_environment(ENTER_WORKTREE_TOOL_NAME, &step_context.environments)?;
    let original_cwd = turn.config.cwd.clone();
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
    let original_info = inspect_worktree_blocking(original_cwd.as_path().to_path_buf()).await?;

    let (worktree_path, branch, name, created) = match (args.name, args.path) {
        (Some(_), Some(_)) => {
            return Err(worktree_model_error(
                "enter_worktree accepts either `name` or `path`, not both".to_string(),
            ));
        }
        (Some(name), None) => {
            ensure_managed_worktree_writes_allowed(
                ENTER_WORKTREE_TOOL_NAME,
                turn.file_system_sandbox_policy(),
                original_cwd.as_path(),
                &original_info,
                &name,
            )?;
            let managed = create_or_reuse_managed_worktree_blocking(
                original_cwd.as_path().to_path_buf(),
                name,
            )
            .await?;
            (
                absolute_path(managed.path, "managed worktree path")?,
                managed.info.current_branch,
                Some(managed.name),
                Some(managed.created),
            )
        }
        (None, Some(path)) => {
            if path.is_empty() {
                return Err(worktree_model_error(
                    "enter_worktree `path` must not be empty".to_string(),
                ));
            }
            let candidate_path = resolve_candidate_path(original_cwd.as_path(), &path);
            let info = validate_managed_same_repository_worktree_blocking(
                original_info.clone(),
                candidate_path,
            )
            .await?;
            ensure_worktree_paths_writable(
                ENTER_WORKTREE_TOOL_NAME,
                turn.file_system_sandbox_policy(),
                original_cwd.as_path(),
                &[original_cwd.as_path().to_path_buf()],
            )?;
            (
                absolute_path(info.repo_root.clone(), "worktree path")?,
                info.current_branch,
                managed_worktree_name(&original_info.common_dir, &info.repo_root),
                None,
            )
        }
        (None, None) => {
            return Err(worktree_model_error(
                "enter_worktree requires either `name` or `path`".to_string(),
            ));
        }
    };
    let worktree_cwd = matching_worktree_cwd(
        original_info.repo_root.as_path(),
        original_cwd.as_path(),
        worktree_path.as_path(),
    )?;
    if let Some(name) = name.as_deref() {
        write_worktree_metadata_blocking(
            original_info.common_dir.clone(),
            name.to_string(),
            original_cwd.clone(),
        )
        .await?;
    }

    let mut updates = cwd_settings_update(
        ENTER_WORKTREE_TOOL_NAME,
        &original_cwd,
        &worktree_cwd,
        &step_context.environments,
    )?;
    updates.workspace_roots = Some(workspace_roots_for_enter(
        &turn.config.workspace_roots,
        &original_cwd,
        &worktree_cwd,
        &worktree_path,
        &original_info.common_dir,
    )?);
    ensure_worktree_output_fits_context(&WorktreeOutput {
        cwd: worktree_cwd.to_string_lossy().to_string(),
        worktree_path: worktree_path.to_string_lossy().to_string(),
        original_cwd: original_cwd.to_string_lossy().to_string(),
        branch: branch.clone(),
        name: name.clone(),
        created,
    })?;
    session
        .update_settings(updates)
        .await
        .map_err(|err| worktree_model_error(format!("failed to enter worktree: {err}")))?;
    session
        .set_active_worktree(ActiveWorktree {
            original_cwd: original_cwd.clone(),
            original_common_dir: original_info.common_dir,
            original_workspace_roots: Some(turn.config.workspace_roots.clone()),
            worktree_path: worktree_path.clone(),
            branch: branch.clone(),
            name: name.clone(),
        })
        .await;
    session
        .send_event(&turn, thread_settings_applied_event(&session).await)
        .await;

    output(WorktreeOutput {
        cwd: worktree_cwd.to_string_lossy().to_string(),
        worktree_path: worktree_path.to_string_lossy().to_string(),
        original_cwd: original_cwd.to_string_lossy().to_string(),
        branch,
        name,
        created,
    })
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
    ensure_local_primary_environment(EXIT_WORKTREE_TOOL_NAME, &step_context.environments)?;
    let (active_worktree_state, current_cwd) =
        active_or_derived_worktree_for_exit(&session, &step_context.environments).await?;
    let is_session_active_worktree =
        matches!(&active_worktree_state, ActiveWorktreeState::Session(_));
    let is_derived_active_worktree =
        matches!(&active_worktree_state, ActiveWorktreeState::Derived(_));
    let active_worktree = active_worktree_state.active_worktree();
    if let Err(err) =
        ensure_current_cwd_matches_active_worktree(active_worktree, current_cwd.as_path()).await
    {
        if is_session_active_worktree {
            session.clear_active_worktree().await;
        }
        return Err(err);
    }

    let mut updates = cwd_settings_update(
        EXIT_WORKTREE_TOOL_NAME,
        &current_cwd,
        &active_worktree.original_cwd,
        &step_context.environments,
    )?;
    if let Some(original_workspace_roots) = active_worktree.original_workspace_roots.clone() {
        updates.workspace_roots = Some(original_workspace_roots);
    } else if is_derived_active_worktree {
        // A cold-resumed session has no in-memory original workspace-root metadata.
        // Preserve current roots so exiting from a managed worktree does not
        // silently rebind write permission to the parent repository.
        updates.workspace_roots = Some(turn.config.workspace_roots.clone());
    }
    session
        .update_settings(updates)
        .await
        .map_err(|err| worktree_model_error(format!("failed to exit worktree: {err}")))?;
    session.clear_active_worktree().await;
    session
        .send_event(&turn, thread_settings_applied_event(&session).await)
        .await;

    let removed = if args.keep {
        false
    } else {
        remove_managed_worktree_blocking(
            active_worktree.original_cwd.as_path().to_path_buf(),
            active_worktree.worktree_path.as_path().to_path_buf(),
        )
        .await?;
        true
    };

    output(ExitWorktreeOutput {
        cwd: active_worktree.original_cwd.to_string_lossy().to_string(),
        worktree_path: active_worktree.worktree_path.to_string_lossy().to_string(),
        original_cwd: active_worktree.original_cwd.to_string_lossy().to_string(),
        branch: active_worktree.branch.clone(),
        name: active_worktree.name.clone(),
        created: None,
        keep: args.keep,
        removed,
    })
}

fn ensure_local_primary_environment(
    tool_name: &str,
    environments: &crate::environment_selection::TurnEnvironmentSnapshot,
) -> Result<(), FunctionCallError> {
    let Some(primary) = environments.primary() else {
        return Err(worktree_model_error(format!(
            "{tool_name} requires a local primary environment that is ready"
        )));
    };
    if primary.environment.is_remote() {
        return Err(worktree_model_error(format!(
            "{tool_name} requires a local primary environment"
        )));
    }
    Ok(())
}

fn cwd_settings_update(
    tool_name: &str,
    current_cwd: &AbsolutePathBuf,
    next_cwd: &AbsolutePathBuf,
    environments: &crate::environment_selection::TurnEnvironmentSnapshot,
) -> Result<SessionSettingsUpdate, FunctionCallError> {
    Ok(SessionSettingsUpdate {
        environments: Some(TurnEnvironmentSelections::new(
            next_cwd.clone(),
            retarget_environment_cwds(tool_name, current_cwd, next_cwd, environments)?,
        )),
        ..Default::default()
    })
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
            && environment.environment_id == primary.environment_id
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
    worktree_path: &AbsolutePathBuf,
    original_common_dir: &Path,
) -> Result<Vec<AbsolutePathBuf>, FunctionCallError> {
    let mut workspace_roots = Vec::with_capacity(current_workspace_roots.len() + 2);
    for root in current_workspace_roots {
        let root = if root == original_cwd {
            worktree_cwd.clone()
        } else {
            root.clone()
        };
        push_unique_workspace_root(&mut workspace_roots, root);
    }
    push_unique_workspace_root(&mut workspace_roots, worktree_path.clone());
    push_unique_workspace_root(
        &mut workspace_roots,
        absolute_path(original_common_dir.to_path_buf(), "git common dir")?,
    );
    Ok(workspace_roots)
}

fn matching_worktree_cwd(
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
    let candidate_cwd = worktree_path.join(relative_cwd);
    let Ok(canonical_candidate_cwd) = candidate_cwd.canonicalize() else {
        return absolute_path(worktree_path.to_path_buf(), "worktree cwd");
    };
    let canonical_worktree_path = canonicalize_for_worktree_check(worktree_path)?;
    if canonical_candidate_cwd.starts_with(&canonical_worktree_path) {
        absolute_path(candidate_cwd, "worktree cwd")
    } else {
        absolute_path(worktree_path.to_path_buf(), "worktree cwd")
    }
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
                "{tool_name} requires filesystem write permission for `{}` before it can manage a worktree; additional permissions or configuration are required",
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

async fn validate_managed_same_repository_worktree_blocking(
    expected: WorktreeInfo,
    candidate_path: PathBuf,
) -> Result<WorktreeInfo, FunctionCallError> {
    git_blocking(move || {
        validate_managed_same_repository_worktree_with_info(&expected, &candidate_path)
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
    let current_info = inspect_worktree_blocking(current_cwd.to_path_buf()).await?;
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
        original_common_dir: current_info.common_dir,
        original_workspace_roots: None,
        worktree_path,
        branch: current_info.current_branch,
        name: Some(name),
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

fn worktree_metadata_path(common_dir: &Path, name: &str) -> PathBuf {
    managed_worktrees_dir(common_dir).join(format!("{name}.{WORKTREE_METADATA_EXTENSION}"))
}

async fn write_worktree_metadata_blocking(
    common_dir: PathBuf,
    name: String,
    original_cwd: AbsolutePathBuf,
) -> Result<(), FunctionCallError> {
    tokio::task::spawn_blocking(move || {
        let metadata = WorktreeMetadata {
            original_cwd: original_cwd.to_string_lossy().to_string(),
        };
        let content = serde_json::to_vec(&metadata).map_err(|err| {
            FunctionCallError::Fatal(format!("failed to serialize worktree metadata: {err}"))
        })?;
        std::fs::write(worktree_metadata_path(&common_dir, &name), content).map_err(|err| {
            worktree_model_error(format!("failed to write managed worktree metadata: {err}"))
        })
    })
    .await
    .map_err(|err| {
        worktree_model_error(format!(
            "worktree metadata write failed to join blocking task: {err}"
        ))
    })?
}

async fn read_worktree_metadata_blocking(
    common_dir: PathBuf,
    name: String,
) -> Result<Option<AbsolutePathBuf>, FunctionCallError> {
    tokio::task::spawn_blocking(move || {
        let path = worktree_metadata_path(&common_dir, &name);
        let content = match std::fs::read(&path) {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(worktree_model_error(format!(
                    "failed to read managed worktree metadata: {err}"
                )));
            }
        };
        let metadata: WorktreeMetadata = serde_json::from_slice(&content).map_err(|err| {
            worktree_model_error(format!(
                "failed to parse managed worktree metadata `{}`: {err}",
                path.display()
            ))
        })?;
        let original_cwd = AbsolutePathBuf::try_from(PathBuf::from(metadata.original_cwd))
            .map_err(|err| {
                worktree_model_error(format!(
                    "managed worktree metadata contains invalid original cwd: {err}"
                ))
            })?;
        Ok(Some(original_cwd))
    })
    .await
    .map_err(|err| {
        worktree_model_error(format!(
            "worktree metadata read failed to join blocking task: {err}"
        ))
    })?
}

async fn ensure_current_cwd_matches_active_worktree(
    active_worktree: &ActiveWorktree,
    current_cwd: &Path,
) -> Result<(), FunctionCallError> {
    let current_info = inspect_worktree_blocking(current_cwd.to_path_buf()).await?;
    if current_info.common_dir != active_worktree.original_common_dir {
        return Err(worktree_model_error(format!(
            "active worktree common git dir changed from `{}` to `{}`; refusing to exit automatically",
            active_worktree.original_common_dir.display(),
            current_info.common_dir.display()
        )));
    }
    let current_repo_root = canonicalize_for_worktree_check(&current_info.repo_root)?;
    let active_path = canonicalize_for_worktree_check(active_worktree.worktree_path.as_path())?;
    if current_repo_root != active_path {
        return Err(worktree_model_error(format!(
            "current cwd `{}` is not inside active worktree `{}`; refusing to exit stale worktree state",
            current_cwd.display(),
            active_worktree.worktree_path.display()
        )));
    }
    if !active_worktree
        .worktree_path
        .as_path()
        .starts_with(managed_worktrees_dir(&active_worktree.original_common_dir))
    {
        return Err(worktree_model_error(format!(
            "active worktree `{}` is not under the managed worktree directory for `{}`",
            active_worktree.worktree_path.display(),
            active_worktree.original_common_dir.display()
        )));
    }
    Ok(())
}

fn canonicalize_for_worktree_check(path: &Path) -> Result<PathBuf, FunctionCallError> {
    path.canonicalize().map_err(|err| {
        worktree_model_error(format!(
            "failed to canonicalize worktree path `{}`: {err}",
            path.display()
        ))
    })
}

fn git_error(err: codex_git_utils::GitToolingError) -> FunctionCallError {
    worktree_model_error(format!("worktree operation failed: {err}"))
}

fn bound_worktree_error(err: FunctionCallError) -> FunctionCallError {
    match err {
        FunctionCallError::RespondToModel(message) => worktree_model_error(message),
        err => err,
    }
}

fn worktree_model_error(message: impl Into<String>) -> FunctionCallError {
    let message = message.into();
    FunctionCallError::RespondToModel(truncate_text(
        &message,
        TruncationPolicy::Bytes(WORKTREE_OUTPUT_MAX_BYTES),
    ))
}

fn output<T: Serialize>(
    output: T,
) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
    ensure_worktree_output_fits_context(&output)?;
    let content = serde_json::to_string(&output).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize worktree output: {err}"))
    })?;
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        content,
        Some(true),
    )))
}

fn ensure_worktree_output_fits_context<T: Serialize>(output: &T) -> Result<(), FunctionCallError> {
    let content = serde_json::to_string(output).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize worktree output: {err}"))
    })?;
    if content.len() > WORKTREE_OUTPUT_MAX_BYTES {
        return Err(worktree_model_error(format!(
            "worktree output exceeds {WORKTREE_OUTPUT_MAX_BYTES} bytes; use a shorter repository path or worktree name"
        )));
    }
    Ok(())
}
