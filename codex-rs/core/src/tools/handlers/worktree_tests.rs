use super::*;

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
