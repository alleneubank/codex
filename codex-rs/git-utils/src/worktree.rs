use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use crate::GitToolingError;
use crate::operations::checkout_filter_config_env_overrides;
use crate::operations::ensure_git_repository;
use crate::operations::resolve_repository_root;
use crate::operations::run_git_for_status;
use crate::operations::run_git_for_stdout;

const MANAGED_WORKTREES_DIR: [&str; 2] = ["codex", "worktrees"];

/// Details Git reports for a checkout that is inside a worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub repo_root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub current_branch: Option<String>,
}

/// Result of creating or reusing a Codex-managed worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedWorktree {
    pub name: String,
    pub path: PathBuf,
    pub created: bool,
    pub info: WorktreeInfo,
}

/// Inspect the repository/worktree containing `path`.
pub fn inspect_worktree(path: &Path) -> Result<WorktreeInfo, GitToolingError> {
    ensure_git_repository(path)?;

    let repo_root = canonicalize_git_path(path, resolve_repository_root(path)?)?;
    let git_dir = canonicalize_git_path(
        path,
        run_git_for_stdout(
            path,
            vec![
                OsString::from("rev-parse"),
                OsString::from("--path-format=absolute"),
                OsString::from("--git-dir"),
            ],
            /*env*/ None,
        )?,
    )?;
    let common_dir = canonicalize_git_path(
        path,
        run_git_for_stdout(
            path,
            vec![
                OsString::from("rev-parse"),
                OsString::from("--path-format=absolute"),
                OsString::from("--git-common-dir"),
            ],
            /*env*/ None,
        )?,
    )?;
    let current_branch = current_branch(path)?;

    Ok(WorktreeInfo {
        repo_root,
        git_dir,
        common_dir,
        current_branch,
    })
}

fn validate_same_repository_worktree_with_info(
    expected: &WorktreeInfo,
    candidate_path: &Path,
) -> Result<WorktreeInfo, GitToolingError> {
    let candidate = inspect_worktree(candidate_path)?;
    if candidate.common_dir != expected.common_dir {
        return Err(GitToolingError::WorktreeRepositoryMismatch {
            path: candidate.repo_root,
            expected_common_dir: expected.common_dir.clone(),
            actual_common_dir: candidate.common_dir,
        });
    }
    Ok(candidate)
}

/// Validate that `candidate_path` is a Codex-managed worktree for `expected`.
pub fn validate_managed_same_repository_worktree_with_info(
    expected: &WorktreeInfo,
    candidate_path: &Path,
) -> Result<WorktreeInfo, GitToolingError> {
    let candidate = validate_same_repository_worktree_with_info(expected, candidate_path)?;
    let managed_base = managed_worktrees_dir(&expected.common_dir);
    let canonical_base = managed_base.canonicalize()?;
    if !candidate.repo_root.starts_with(&canonical_base) {
        return Err(GitToolingError::ManagedWorktreePathEscapes {
            path: candidate.repo_root,
            base: managed_base,
        });
    }
    Ok(candidate)
}

/// Create or reuse `codex/worktrees/<name>` under the git common dir for `repository_path`.
pub fn create_or_reuse_managed_worktree(
    repository_path: &Path,
    name: &str,
) -> Result<ManagedWorktree, GitToolingError> {
    validate_managed_worktree_name(name)?;
    let source = inspect_worktree(repository_path)?;
    let managed_base = managed_worktrees_dir(&source.common_dir);
    let canonical_base = ensure_managed_base(&source, &managed_base)?;

    let target_path = canonical_base.join(name);
    let created = if std::fs::symlink_metadata(&target_path).is_ok() {
        ensure_managed_target_stays_under_base(&target_path, &canonical_base)?;
        false
    } else {
        add_managed_worktree(&source.repo_root, &target_path, name)?;
        true
    };

    let info = validate_same_repository_worktree_with_info(&source, &target_path)?;
    Ok(ManagedWorktree {
        name: name.to_string(),
        path: target_path,
        created,
        info,
    })
}

/// Remove a Codex-managed worktree from the same repository as `repository_path`.
pub fn remove_managed_worktree(
    repository_path: &Path,
    worktree_path: &Path,
) -> Result<(), GitToolingError> {
    let source = inspect_worktree(repository_path)?;
    let candidate = validate_managed_same_repository_worktree_with_info(&source, worktree_path)?;
    run_git_for_status(
        &source.repo_root,
        vec![
            OsString::from("worktree"),
            OsString::from("remove"),
            candidate.repo_root.as_os_str().to_os_string(),
        ],
        /*env*/ None,
    )
}

/// Return the Codex-managed worktree directory under a repository git common dir.
pub fn managed_worktrees_dir(common_dir: &Path) -> PathBuf {
    let mut base = common_dir.to_path_buf();
    for component in MANAGED_WORKTREES_DIR {
        base.push(component);
    }
    base
}

/// Return the path for a named managed worktree under `common_dir`.
pub fn managed_worktree_path(common_dir: &Path, name: &str) -> Result<PathBuf, GitToolingError> {
    validate_managed_worktree_name(name)?;
    Ok(managed_worktrees_dir(common_dir).join(name))
}

fn ensure_managed_base(
    source: &WorktreeInfo,
    managed_base: &Path,
) -> Result<PathBuf, GitToolingError> {
    let canonical_common_dir = source.common_dir.canonicalize()?;
    let Some(managed_parent) = managed_base.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "managed worktree base must have parent",
        )
        .into());
    };
    let canonical_parent = ensure_managed_dir(managed_parent, &canonical_common_dir)?;
    let canonical_base = ensure_managed_dir(managed_base, &canonical_parent)?;
    if !canonical_base.starts_with(&canonical_common_dir) {
        return Err(GitToolingError::ManagedWorktreePathEscapes {
            path: canonical_base,
            base: managed_base.to_path_buf(),
        });
    }
    Ok(canonical_base)
}

fn ensure_managed_dir(path: &Path, canonical_parent: &Path) -> Result<PathBuf, GitToolingError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            let canonical_path = path.canonicalize()?;
            if canonical_path.starts_with(canonical_parent) {
                Ok(canonical_path)
            } else {
                Err(GitToolingError::ManagedWorktreePathEscapes {
                    path: canonical_path,
                    base: path.to_path_buf(),
                })
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(path)?;
            let canonical_path = path.canonicalize()?;
            if canonical_path.starts_with(canonical_parent) {
                Ok(canonical_path)
            } else {
                Err(GitToolingError::ManagedWorktreePathEscapes {
                    path: canonical_path,
                    base: path.to_path_buf(),
                })
            }
        }
        Err(err) => Err(err.into()),
    }
}

fn validate_managed_worktree_name(name: &str) -> Result<(), GitToolingError> {
    let reason = if name.is_empty() {
        Some("name must not be empty")
    } else if name.starts_with('-') {
        Some("name must not start with '-'")
    } else if name.contains(['/', '\\']) {
        Some("name must not contain path separators")
    } else if name.contains('\0') {
        Some("name must not contain NUL bytes")
    } else if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        Some("name must contain only ASCII letters, digits, '.', '_', or '-'")
    } else {
        let path = Path::new(name);
        let mut components = path.components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(component)), None) if component == OsStr::new(name) => None,
            _ => Some("name must map to exactly one child path"),
        }
    };

    if let Some(reason) = reason {
        return Err(GitToolingError::InvalidWorktreeName {
            name: name.to_string(),
            reason: reason.to_string(),
        });
    }

    Ok(())
}

fn ensure_managed_target_stays_under_base(
    target_path: &Path,
    canonical_base: &Path,
) -> Result<(), GitToolingError> {
    let canonical_target = target_path.canonicalize()?;
    if canonical_target.starts_with(canonical_base) {
        Ok(())
    } else {
        Err(GitToolingError::ManagedWorktreePathEscapes {
            path: canonical_target,
            base: canonical_base.to_path_buf(),
        })
    }
}

fn add_managed_worktree(
    repo_root: &Path,
    target_path: &Path,
    name: &str,
) -> Result<(), GitToolingError> {
    let checkout_filter_env = checkout_filter_config_env_overrides(repo_root)?;
    if branch_exists(repo_root, name)? {
        run_git_for_status(
            repo_root,
            vec![
                OsString::from("worktree"),
                OsString::from("add"),
                target_path.as_os_str().to_os_string(),
                OsString::from(name),
            ],
            Some(&checkout_filter_env),
        )
    } else {
        run_git_for_status(
            repo_root,
            vec![
                OsString::from("worktree"),
                OsString::from("add"),
                OsString::from("-b"),
                OsString::from(name),
                target_path.as_os_str().to_os_string(),
                OsString::from("HEAD"),
            ],
            Some(&checkout_filter_env),
        )
    }
}

fn branch_exists(repo_root: &Path, name: &str) -> Result<bool, GitToolingError> {
    match run_git_for_status(
        repo_root,
        vec![
            OsString::from("rev-parse"),
            OsString::from("--verify"),
            OsString::from(format!("refs/heads/{name}")),
        ],
        /*env*/ None,
    ) {
        Ok(()) => Ok(true),
        Err(GitToolingError::GitCommand { .. }) => Ok(false),
        Err(err) => Err(err),
    }
}

fn current_branch(path: &Path) -> Result<Option<String>, GitToolingError> {
    let branch = run_git_for_stdout(
        path,
        vec![
            OsString::from("rev-parse"),
            OsString::from("--abbrev-ref"),
            OsString::from("HEAD"),
        ],
        /*env*/ None,
    )?;
    if branch == "HEAD" || branch.is_empty() {
        Ok(None)
    } else {
        Ok(Some(branch))
    }
}

fn canonicalize_git_path(
    command_dir: &Path,
    path: impl Into<PathBuf>,
) -> Result<PathBuf, GitToolingError> {
    let path = path.into();
    let absolute = if path.is_absolute() {
        path
    } else {
        command_dir.join(path)
    };
    Ok(absolute.canonicalize()?)
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
