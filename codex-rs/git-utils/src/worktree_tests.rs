//! Validates repository identity and discovery across linked checkout layouts.

use super::*;
use pretty_assertions::assert_eq;

fn repository_with_linked_checkout() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().expect("temporary repository");
    let primary = root.path().join("primary");
    let linked = root.path().join("linked");
    let admin = primary.join(".git").join("worktrees").join("linked");
    fs::create_dir_all(primary.join("nested")).expect("primary nested directory");
    fs::create_dir_all(linked.join("nested")).expect("linked nested directory");
    fs::create_dir_all(&admin).expect("worktree administrative directory");
    fs::write(primary.join(".git/HEAD"), "ref: refs/heads/main\n")
        .expect("primary repository HEAD");
    fs::write(admin.join("commondir"), "../..\n").expect("common directory");
    fs::write(
        admin.join("gitdir"),
        format!("{}\n", linked.join(".git").display()),
    )
    .expect("linked checkout backlink");
    fs::write(
        linked.join(".git"),
        format!("gitdir: {}\n", admin.display()),
    )
    .expect("linked checkout git file");
    (root, primary, linked)
}

#[test]
fn repository_identity_is_shared_by_primary_and_linked_checkouts() {
    let (_root, primary, linked) = repository_with_linked_checkout();
    let primary_cwd = canonicalize_native(primary.join("nested")).expect("primary cwd");
    let linked_cwd = canonicalize_native(linked.join("nested")).expect("linked cwd");
    let expected = RepositoryIdentity {
        common_dir: AbsolutePathBuf::from_absolute_path_checked(
            canonicalize_native(primary.join(".git")).expect("common git directory"),
        )
        .expect("absolute common directory"),
        relative_cwd: PathBuf::from("nested"),
        primary_root: AbsolutePathBuf::from_absolute_path_checked(
            canonicalize_native(&primary).expect("primary checkout"),
        )
        .expect("absolute primary checkout"),
    };

    assert_eq!(repository_identity(&primary_cwd), Some(expected.clone()));
    assert_eq!(repository_identity(&linked_cwd), Some(expected));
    assert_eq!(
        linked_worktree_cwds(&linked_cwd),
        Some(vec![linked_cwd, primary_cwd])
    );
}

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;
use tempfile::tempdir;

use super::WorktreeInfo;
use super::create_or_reuse_managed_worktree;
use super::inspect_worktree;
use super::managed_worktree_path;
use super::remove_created_managed_worktree;
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
    assert_eq!(created.created_branch, Some("codex-test".to_string()));
    assert_eq!(created.info.repo_root, expected_path);
    assert_eq!(created.info.current_branch, Some("codex-test".to_string()));

    let reused = create_or_reuse_managed_worktree(repo.path(), "codex-test")?;
    assert_eq!(reused.name, "codex-test");
    assert_eq!(reused.path, expected_path);
    assert!(!reused.created);
    assert_eq!(reused.created_branch, None);
    assert_eq!(reused.info.common_dir, created.info.common_dir);

    Ok(())
}

#[test]
fn managed_worktree_from_linked_checkout_preserves_sibling_identity() -> Result<(), GitToolingError>
{
    let repo = init_repo()?;
    std::fs::create_dir(repo.path().join("nested"))?;
    std::fs::write(repo.path().join("nested/.keep"), "nested\n")?;
    run_git(repo.path(), &["add", "nested/.keep"]);
    run_git(
        repo.path(),
        &[
            "-c",
            "user.name=Tester",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "add nested checkout directory",
        ],
    );
    let sibling_root = tempfile::tempdir()?;
    let sibling = sibling_root.path().join("checkout");
    run_git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-b",
            "sibling-branch",
            sibling.to_str().expect("utf-8 sibling path"),
            "HEAD",
        ],
    );
    let sibling_cwd = sibling.join("nested").canonicalize()?;
    let created = create_or_reuse_managed_worktree(&sibling_cwd, "codex-linked")?;
    let managed_cwd = created.path.join("nested");
    let sibling_identity = repository_identity(&sibling_cwd).expect("sibling identity");
    assert_eq!(
        repository_identity(&managed_cwd),
        Some(sibling_identity.clone())
    );
    let primary_cwd = repo.path().join("nested").canonicalize()?;
    assert_eq!(
        linked_worktree_cwds(&managed_cwd),
        Some(vec![managed_cwd.clone(), primary_cwd, sibling_cwd.clone()])
    );
    let adopted = create_or_reuse_managed_worktree(repo.path(), "codex-linked")?;
    assert!(!adopted.created);
    assert_eq!(adopted.path, created.path);
    assert_eq!(adopted.info.common_dir, created.info.common_dir);
    remove_created_managed_worktree(
        repo.path(),
        &created.path,
        created.created_branch.as_deref(),
    )?;
    assert!(sibling_cwd.is_dir());
    assert_eq!(repository_identity(&sibling_cwd), Some(sibling_identity));
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
fn remove_created_managed_worktree_removes_new_branch() -> Result<(), GitToolingError> {
    let repo = init_repo()?;
    let managed = create_or_reuse_managed_worktree(repo.path(), "codex-rollback")?;

    remove_created_managed_worktree(
        repo.path(),
        &managed.path,
        managed.created_branch.as_deref(),
    )?;

    assert!(!managed.path.exists());
    let branch = Command::new("git")
        .current_dir(repo.path())
        .args(["show-ref", "--verify", "refs/heads/codex-rollback"])
        .output()
        .expect("git show-ref");
    assert!(!branch.status.success());
    Ok(())
}

#[test]
fn remove_created_managed_worktree_preserves_reused_branch() -> Result<(), GitToolingError> {
    let repo = init_repo()?;
    let branch = "codex-existing-rollback";
    let status = Command::new("git")
        .current_dir(repo.path())
        .args(["branch", branch])
        .status()
        .expect("git branch");
    assert!(status.success(), "git branch should succeed");
    let managed = create_or_reuse_managed_worktree(repo.path(), branch)?;
    assert_eq!(managed.created_branch, None);

    remove_created_managed_worktree(repo.path(), &managed.path, /*created_branch*/ None)?;

    assert!(!managed.path.exists());
    let branch = Command::new("git")
        .current_dir(repo.path())
        .args(["show-ref", "--verify", "refs/heads/codex-existing-rollback"])
        .output()
        .expect("git show-ref");
    assert!(branch.status.success());
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
fn linked_worktree_discovery_preserves_logical_working_directory_aliases() {
    let (root, primary, linked) = repository_with_linked_checkout();
    let alias = root.path().join("primary-alias");
    std::os::unix::fs::symlink(&primary, &alias).expect("checkout alias");
    let logical_cwd = alias.join("nested");
    let canonical_cwd = fs::canonicalize(primary.join("nested")).expect("primary cwd");
    let linked_cwd = fs::canonicalize(linked.join("nested")).expect("linked cwd");

    assert_eq!(
        linked_worktree_cwds(&logical_cwd),
        Some(vec![logical_cwd, canonical_cwd, linked_cwd])
    );
}

#[test]
fn linked_worktree_discovery_rejects_mismatched_backlinks() {
    let (_root, primary, linked) = repository_with_linked_checkout();
    let primary_cwd = canonicalize_native(primary.join("nested")).expect("primary cwd");
    let admin = primary.join(".git").join("worktrees").join("linked");
    fs::write(
        admin.join("gitdir"),
        primary.join(".git").display().to_string(),
    )
    .expect("invalid worktree backlink");

    assert_eq!(repository_identity(&linked.join("nested")), None);
    assert_eq!(linked_worktree_cwds(&primary_cwd), Some(vec![primary_cwd]));
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
fn linked_worktree_discovery_rejects_relative_directory_symlink_escapes() {
    let (root, primary, linked) = repository_with_linked_checkout();
    let primary_cwd = fs::canonicalize(primary.join("nested")).expect("primary cwd");
    let outside = root.path().join("outside");
    fs::create_dir(&outside).expect("outside directory");
    fs::remove_dir(linked.join("nested")).expect("remove linked nested directory");
    std::os::unix::fs::symlink(&outside, linked.join("nested")).expect("escaping symlink");

    assert_eq!(linked_worktree_cwds(&primary_cwd), Some(vec![primary_cwd]));
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
