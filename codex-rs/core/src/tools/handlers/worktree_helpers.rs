use super::*;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::boxed_tool_output;
use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_output_truncation::truncate_text;
use serde::Serialize;

pub(super) fn worktree_metadata_path(common_dir: &Path, name: &str) -> PathBuf {
    managed_worktrees_dir(common_dir).join(format!("{name}.{WORKTREE_METADATA_EXTENSION}"))
}

pub(super) async fn write_worktree_metadata_blocking(
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

pub(super) async fn read_worktree_metadata_blocking(
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

pub(super) async fn ensure_current_cwd_matches_active_worktree(
    active_worktree: &ActiveWorktree,
    current_cwd: &Path,
) -> Result<(), FunctionCallError> {
    let original_common_dir = match &active_worktree.ownership {
        ActiveWorktreeOwnership::Adopted => {
            let current_cwd = canonicalize_for_worktree_check(current_cwd)?;
            let active_path =
                canonicalize_for_worktree_check(active_worktree.worktree_path.as_path())?;
            if !current_cwd.starts_with(&active_path) {
                return Err(worktree_model_error(format!(
                    "current cwd `{}` is not inside active adopted workdir `{}`; refusing to exit stale worktree state",
                    current_cwd.display(),
                    active_worktree.worktree_path.display()
                )));
            }
            return Ok(());
        }
        ActiveWorktreeOwnership::ManagedByCodex(original_common_dir) => original_common_dir,
    };
    let current_info = inspect_worktree_blocking(current_cwd.to_path_buf()).await?;
    if current_info.common_dir != *original_common_dir {
        return Err(worktree_model_error(format!(
            "active worktree common git dir changed from `{}` to `{}`; refusing to exit automatically",
            original_common_dir.display(),
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
        .starts_with(managed_worktrees_dir(original_common_dir))
    {
        return Err(worktree_model_error(format!(
            "active worktree `{}` is not under the managed worktree directory for `{}`",
            active_worktree.worktree_path.display(),
            original_common_dir.display()
        )));
    }
    Ok(())
}

pub(super) async fn inspect_optional_worktree_blocking(
    path: PathBuf,
) -> Result<Option<WorktreeInfo>, FunctionCallError> {
    git_blocking(move || match inspect_worktree(&path) {
        Ok(info) => Ok(Some(info)),
        Err(codex_git_utils::GitToolingError::NotAGitRepository { .. }) => Ok(None),
        Err(err) => Err(err),
    })
    .await
}

pub(super) fn canonicalize_for_worktree_check(path: &Path) -> Result<PathBuf, FunctionCallError> {
    path.canonicalize().map_err(|err| {
        worktree_model_error(format!(
            "failed to canonicalize worktree path `{}`: {err}",
            path.display()
        ))
    })
}

pub(super) fn git_error(err: codex_git_utils::GitToolingError) -> FunctionCallError {
    worktree_model_error(format!("worktree operation failed: {err}"))
}

pub(super) fn bound_worktree_error(err: FunctionCallError) -> FunctionCallError {
    match err {
        FunctionCallError::RespondToModel(message) => worktree_model_error(message),
        err => err,
    }
}

pub(super) fn worktree_model_error(message: impl Into<String>) -> FunctionCallError {
    let message = message.into();
    FunctionCallError::RespondToModel(truncate_text(
        &message,
        TruncationPolicy::Bytes(WORKTREE_OUTPUT_MAX_BYTES),
    ))
}

pub(super) fn output<T: Serialize>(
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

pub(super) fn ensure_worktree_output_fits_context<T: Serialize>(
    output: &T,
) -> Result<(), FunctionCallError> {
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
