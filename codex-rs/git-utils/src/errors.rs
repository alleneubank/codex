use std::path::PathBuf;
use std::process::ExitStatus;
use std::string::FromUtf8Error;

use thiserror::Error;
use walkdir::Error as WalkdirError;

/// Errors returned while managing git worktree snapshots.
#[derive(Debug, Error)]
pub enum GitToolingError {
    #[error("git command `{command}` failed with status {status}: {stderr}")]
    GitCommand {
        command: String,
        status: ExitStatus,
        stderr: String,
    },
    #[error("git command `{command}` produced non-UTF-8 output")]
    GitOutputUtf8 {
        command: String,
        #[source]
        source: FromUtf8Error,
    },
    #[error("{path:?} is not a git repository")]
    NotAGitRepository { path: PathBuf },
    #[error("invalid managed worktree name {name:?}: {reason}")]
    InvalidWorktreeName { name: String, reason: String },
    #[error("path {path:?} escapes managed worktree directory {base:?}")]
    ManagedWorktreePathEscapes { path: PathBuf, base: PathBuf },
    #[error(
        "worktree {path:?} belongs to git common dir {actual_common_dir:?}, expected {expected_common_dir:?}"
    )]
    WorktreeRepositoryMismatch {
        path: PathBuf,
        expected_common_dir: PathBuf,
        actual_common_dir: PathBuf,
    },
    #[error("path {path:?} must be relative to the repository root")]
    NonRelativePath { path: PathBuf },
    #[error("path {path:?} escapes the repository root")]
    PathEscapesRepository { path: PathBuf },
    #[error("failed to process path inside worktree")]
    PathPrefix(#[from] std::path::StripPrefixError),
    #[error(transparent)]
    Walkdir(#[from] WalkdirError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
