use super::*;
use std::fs;

#[test]
fn worktree_output_byte_cap_is_conservative_for_token_budget() {
    assert_eq!(WORKTREE_OUTPUT_MAX_BYTES, WORKTREE_OUTPUT_MAX_TOKENS);
}

#[test]
fn managed_worktree_output_is_preflighted_before_creation() {
    let temp_dir = tempfile::tempdir().expect("temporary directory");
    let original_repo = temp_dir.path().join("repo");
    let original_cwd_path = original_repo.join("src").join("app");
    fs::create_dir_all(&original_cwd_path).expect("create original repository directory");
    let original_cwd =
        AbsolutePathBuf::from_absolute_path(original_cwd_path).expect("absolute original cwd");
    let original_info = WorktreeInfo {
        repo_root: original_repo.clone(),
        git_dir: original_repo.join(".git"),
        common_dir: original_repo.join(".git"),
        current_branch: Some("main".to_string()),
    };
    let name = "x".repeat(WORKTREE_OUTPUT_MAX_BYTES);
    let managed_path = managed_worktree_path(&original_info.common_dir, &name)
        .expect("the synthetic name should be valid");
    let anticipated_cwd = anticipated_worktree_cwd(
        &original_info.repo_root,
        original_cwd.as_path(),
        &managed_path,
    )
    .expect("anticipated worktree cwd");
    let expected_cwd = managed_path.join("src/app");
    assert_eq!(anticipated_cwd.as_path(), expected_cwd.as_path());

    let err = preflight_managed_worktree_output(&original_info, &original_cwd, &name)
        .expect_err("an oversized managed worktree output should fail before creation");
    assert!(
        matches!(err, FunctionCallError::RespondToModel(message) if message.contains("exceeds"))
    );
    assert!(!managed_path.exists());
}

#[test]
fn entering_worktree_does_not_grant_git_common_dir_write_access() {
    let temp_dir = tempfile::tempdir().expect("temporary directory");
    let original_repo = temp_dir.path().join("repo");
    let common_dir = original_repo.join(".git");
    let worktree_path = temp_dir.path().join("worktree");
    fs::create_dir_all(&common_dir).expect("create common git directory");
    fs::create_dir_all(&worktree_path).expect("create managed worktree directory");
    let original_cwd =
        AbsolutePathBuf::from_absolute_path(original_repo).expect("absolute original cwd");
    let worktree =
        AbsolutePathBuf::from_absolute_path(worktree_path.clone()).expect("absolute worktree path");

    let workspace_roots = workspace_roots_for_enter(
        std::slice::from_ref(&original_cwd),
        &original_cwd,
        &worktree,
        &worktree,
    );
    assert!(
        !workspace_roots
            .contains(&AbsolutePathBuf::from_absolute_path(common_dir.clone()).unwrap())
    );
    let policy = FileSystemSandboxPolicy::workspace_write(&workspace_roots, true, true);

    assert!(!policy.can_write_path_with_cwd(&common_dir.join("config"), worktree_path.as_path()));
}

#[test]
fn git_error_output_is_bounded() {
    let err = codex_git_utils::GitToolingError::InvalidWorktreeName {
        name: "x".repeat(WORKTREE_OUTPUT_MAX_BYTES * 2),
        reason: "bad name".to_string(),
    };

    let FunctionCallError::RespondToModel(message) = git_error(err) else {
        panic!("git errors should be reported to the model");
    };

    assert!(message.len() < WORKTREE_OUTPUT_MAX_BYTES + 512);
    assert!(message.contains("truncated"));
}

#[test]
fn write_permission_error_output_is_bounded() {
    let file_system_sandbox_policy = FileSystemSandboxPolicy::workspace_write(
        &[],
        /*exclude_tmpdir_env_var*/ true,
        /*exclude_slash_tmp*/ true,
    );
    let long_path = PathBuf::from(format!(
        "/blocked/{}",
        "x".repeat(WORKTREE_OUTPUT_MAX_BYTES * 2)
    ));

    let err = ensure_worktree_paths_writable(
        ENTER_WORKTREE_TOOL_NAME,
        file_system_sandbox_policy,
        Path::new("/workspace"),
        &[long_path],
    )
    .expect_err("write permission should be denied");
    let FunctionCallError::RespondToModel(message) = err else {
        panic!("permission errors should be reported to the model");
    };

    assert!(message.len() < WORKTREE_OUTPUT_MAX_BYTES + 512);
    assert!(message.contains("truncated"));
}
