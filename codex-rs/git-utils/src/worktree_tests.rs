use std::path::Path;
use std::process::Command;

use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tempfile::tempdir;

use super::WorktreeInfo;
use super::create_or_reuse_managed_worktree;
use super::inspect_worktree;
use super::managed_worktree_path;
use super::remove_managed_worktree;
use crate::GitToolingError;

fn init_repo() -> Result<TempDir, GitToolingError> {
    let temp = tempdir()?;
    run_git(temp.path(), &["init", "--initial-branch=main"]);
    run_git(temp.path(), &["config", "core.autocrlf", "false"]);
    std::fs::write(temp.path().join("README.md"), "hello\n")?;
    run_git(temp.path(), &["add", "README.md"]);
    run_git(
        temp.path(),
        &[
            "-c",
            "user.name=Tester",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial commit",
        ],
    );
    Ok(temp)
}

fn run_git(repo_path: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo_path)
        .args(args)
        .status()
        .expect("git command");
    assert!(status.success(), "git command failed: {args:?}");
}

fn validate_same_repository_worktree(
    repository_path: &Path,
    candidate_path: &Path,
) -> Result<WorktreeInfo, GitToolingError> {
    let expected = inspect_worktree(repository_path)?;
    super::validate_same_repository_worktree_with_info(&expected, candidate_path)
}

fn validate_managed_same_repository_worktree(
    repository_path: &Path,
    candidate_path: &Path,
) -> Result<WorktreeInfo, GitToolingError> {
    let expected = inspect_worktree(repository_path)?;
    super::validate_managed_same_repository_worktree_with_info(&expected, candidate_path)
}

#[test]
fn inspect_worktree_returns_repository_paths_and_branch() -> Result<(), GitToolingError> {
    let repo = init_repo()?;
    let subdir = repo.path().join("subdir");
    std::fs::create_dir(&subdir)?;

    let info = inspect_worktree(&subdir)?;

    assert_eq!(
        info,
        WorktreeInfo {
            repo_root: repo.path().canonicalize()?,
            git_dir: repo.path().join(".git").canonicalize()?,
            common_dir: repo.path().join(".git").canonicalize()?,
            current_branch: Some("main".to_string()),
        }
    );
    Ok(())
}

#[test]
fn create_or_reuse_managed_worktree_under_codex_dir() -> Result<(), GitToolingError> {
    let repo = init_repo()?;

    let created = create_or_reuse_managed_worktree(repo.path(), "codex-test")?;
    let source = inspect_worktree(repo.path())?;
    let expected_path = source
        .common_dir
        .join("codex/worktrees/codex-test")
        .canonicalize()?;
    assert_eq!(created.name, "codex-test");
    assert_eq!(created.path, expected_path);
    assert!(created.created);
    assert_eq!(created.info.repo_root, expected_path);
    assert_eq!(created.info.current_branch, Some("codex-test".to_string()));

    let reused = create_or_reuse_managed_worktree(repo.path(), "codex-test")?;
    assert_eq!(reused.name, "codex-test");
    assert_eq!(reused.path, expected_path);
    assert!(!reused.created);
    assert_eq!(reused.info.common_dir, created.info.common_dir);

    Ok(())
}

#[test]
fn create_managed_worktree_leaves_source_repo_clean() -> Result<(), GitToolingError> {
    let repo = init_repo()?;

    create_or_reuse_managed_worktree(repo.path(), "codex-clean")?;

    let status = Command::new("git")
        .current_dir(repo.path())
        .args(["status", "--short"])
        .output()
        .expect("git status");
    assert!(status.status.success(), "git status failed");
    assert_eq!(String::from_utf8_lossy(&status.stdout), "");
    Ok(())
}

#[test]
fn remove_managed_worktree_removes_clean_worktree() -> Result<(), GitToolingError> {
    let repo = init_repo()?;
    let managed = create_or_reuse_managed_worktree(repo.path(), "codex-remove")?;

    remove_managed_worktree(repo.path(), &managed.path)?;

    assert!(!managed.path.exists());
    let worktree_list = Command::new("git")
        .current_dir(repo.path())
        .args(["worktree", "list", "--porcelain"])
        .output()
        .expect("git worktree list");
    assert!(worktree_list.status.success(), "git worktree list failed");
    assert!(
        !String::from_utf8_lossy(&worktree_list.stdout)
            .contains(&managed.path.to_string_lossy().to_string()),
        "removed worktree should not appear in git worktree list"
    );
    Ok(())
}

#[test]
fn remove_managed_worktree_rejects_dirty_worktree() -> Result<(), GitToolingError> {
    let repo = init_repo()?;
    let managed = create_or_reuse_managed_worktree(repo.path(), "codex-dirty")?;
    std::fs::write(managed.path.join("dirty.txt"), "dirty\n")?;

    let err = remove_managed_worktree(repo.path(), &managed.path)
        .expect_err("dirty worktree must not be removed");

    assert!(matches!(err, GitToolingError::GitCommand { .. }));
    assert!(managed.path.exists());
    Ok(())
}

#[test]
fn create_managed_worktree_disables_checkout_filter_helpers() -> Result<(), GitToolingError> {
    let repo = init_repo()?;
    std::fs::write(repo.path().join(".gitattributes"), "*.txt filter=evil\n")?;
    std::fs::write(repo.path().join("filtered.txt"), "filtered\n")?;
    run_git(repo.path(), &["add", ".gitattributes", "filtered.txt"]);
    run_git(
        repo.path(),
        &[
            "-c",
            "user.name=Tester",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "add filtered file",
        ],
    );
    run_git(repo.path(), &["config", "filter.evil.smudge", "git false"]);
    run_git(repo.path(), &["config", "filter.evil.process", "git false"]);
    run_git(repo.path(), &["config", "filter.evil.required", "true"]);

    let created = create_or_reuse_managed_worktree(repo.path(), "codex-filtered")?;

    assert!(created.path.join("filtered.txt").exists());
    Ok(())
}

#[test]
fn validate_same_repository_rejects_cross_repo_worktree() -> Result<(), GitToolingError> {
    let repo = init_repo()?;
    let other = init_repo()?;

    let err = validate_same_repository_worktree(repo.path(), other.path())
        .expect_err("cross-repo worktree must be rejected");

    assert!(matches!(
        err,
        GitToolingError::WorktreeRepositoryMismatch { .. }
    ));
    Ok(())
}

#[test]
fn validate_managed_same_repository_rejects_unmanaged_worktree() -> Result<(), GitToolingError> {
    let repo = init_repo()?;
    let source = inspect_worktree(repo.path())?;
    std::fs::create_dir_all(source.common_dir.join("codex/worktrees"))?;
    let unmanaged = repo.path().with_file_name(format!(
        "{}-unmanaged",
        repo.path()
            .file_name()
            .expect("tempdir name")
            .to_string_lossy()
    ));
    run_git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-b",
            "unmanaged-branch",
            unmanaged.to_str().expect("utf-8 path"),
            "HEAD",
        ],
    );

    let err = validate_managed_same_repository_worktree(repo.path(), &unmanaged)
        .expect_err("unmanaged worktree must be rejected");

    assert!(matches!(
        err,
        GitToolingError::ManagedWorktreePathEscapes { .. }
    ));
    Ok(())
}

#[test]
fn create_managed_worktree_rejects_invalid_names() -> Result<(), GitToolingError> {
    let repo = init_repo()?;
    for name in [
        "",
        "..",
        "feature/name",
        "feature\\name",
        "-flag",
        "has space",
    ] {
        let err = create_or_reuse_managed_worktree(repo.path(), name)
            .expect_err("invalid name must be rejected");
        assert!(
            matches!(err, GitToolingError::InvalidWorktreeName { .. }),
            "name: {name}, error: {err}"
        );
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn create_managed_worktree_rejects_symlink_escape() -> Result<(), GitToolingError> {
    let repo = init_repo()?;
    let source = inspect_worktree(repo.path())?;
    let managed_dir = source.common_dir.join("codex/worktrees");
    let outside = repo.path().join("outside");
    std::fs::create_dir_all(&managed_dir)?;
    std::fs::create_dir(&outside)?;
    std::os::unix::fs::symlink(&outside, managed_dir.join("escape"))?;

    let err = create_or_reuse_managed_worktree(repo.path(), "escape")
        .expect_err("symlink escape must be rejected");

    assert!(matches!(
        err,
        GitToolingError::ManagedWorktreePathEscapes { .. }
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn create_managed_worktree_rejects_symlinked_managed_parent() -> Result<(), GitToolingError> {
    let repo = init_repo()?;
    let source = inspect_worktree(repo.path())?;
    let outside = repo.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::os::unix::fs::symlink(&outside, source.common_dir.join("codex"))?;

    let err = create_or_reuse_managed_worktree(repo.path(), "escape")
        .expect_err("symlinked managed parent must be rejected");

    assert!(matches!(
        err,
        GitToolingError::ManagedWorktreePathEscapes { .. }
    ));
    assert!(!outside.join("worktrees").exists());
    Ok(())
}

#[test]
fn managed_worktree_path_validates_name() -> Result<(), GitToolingError> {
    let repo = init_repo()?;
    let source = inspect_worktree(repo.path())?;

    assert_eq!(
        managed_worktree_path(&source.common_dir, "codex-test")?,
        source.common_dir.join("codex/worktrees/codex-test")
    );
    assert!(matches!(
        managed_worktree_path(&source.common_dir, "../escape"),
        Err(GitToolingError::InvalidWorktreeName { .. })
    ));
    Ok(())
}
