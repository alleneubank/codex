use crate::environment_selection::TurnEnvironmentState;
use crate::function_tool::FunctionCallError;
use crate::session::ActiveWorktree;
use crate::session::ActiveWorktreeOwnership;
use crate::session::SessionSettingsUpdate;
use crate::session::acquire_thread_settings_persistence_lock;
use crate::session::thread_settings_applied_event_from_snapshot;
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

#[path = "worktree_discovery.rs"]
mod worktree_discovery;
#[path = "worktree_enter.rs"]
mod worktree_enter;
#[path = "worktree_exit.rs"]
mod worktree_exit;
#[path = "worktree_helpers.rs"]
mod worktree_helpers;
use worktree_discovery::active_or_derived_worktree;
use worktree_discovery::active_or_derived_worktree_for_exit;
use worktree_discovery::managed_worktree_name;
use worktree_enter::enter_worktree;
use worktree_exit::exit_worktree;
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
    #[serde(default)]
    owner_thread_id: Option<String>,
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

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
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

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(exit_worktree(invocation))
    }
}

impl CoreToolRuntime for ExitWorktreeHandler {}

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
    workspace_roots_update: &PrimaryWorkspaceRootsUpdate,
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
        worktree_transition: true,
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
    file_system_sandbox_policy: &FileSystemSandboxPolicy,
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
    file_system_sandbox_policy: &FileSystemSandboxPolicy,
    cwd: &Path,
    paths: &[PathBuf],
) -> Result<(), FunctionCallError> {
    let has_full_disk_write_access = file_system_sandbox_policy.has_full_disk_write_access();
    let writable_roots = file_system_sandbox_policy.get_writable_roots_with_cwd(cwd);
    for path in paths {
        let canonical_path = canonicalize_for_worktree_check(path)?;
        if !has_full_disk_write_access
            && !writable_roots
                .iter()
                .any(|root| root.is_path_writable(&canonical_path))
        {
            return Err(worktree_model_error(format!(
                "{tool_name} requires filesystem write permission for `{}` before it can change the session workdir; additional permissions or configuration are required",
                canonical_path.display()
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
