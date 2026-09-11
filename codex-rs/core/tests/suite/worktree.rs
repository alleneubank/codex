use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use anyhow::Context;
use anyhow::Result;
use codex_features::Feature;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::NetworkSandboxPolicy;
use core_test_support::PathBufExt;
use core_test_support::hooks::trust_discovered_hooks;
use core_test_support::responses::ev_apply_patch_custom_tool_call;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_function_call_with_namespace;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once_match;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::skip_if_remote;
use core_test_support::test_codex::test_codex;
use serde_json::Value;
use serde_json::json;

#[cfg(unix)]
use std::os::unix::fs::symlink as symlink_dir;
#[cfg(windows)]
use std::os::windows::fs::symlink_dir;

const ENTER_WORKTREE_TOOL_NAME: &str = "enter_worktree";
const EXIT_WORKTREE_TOOL_NAME: &str = "exit_worktree";
const MULTI_AGENT_V1_NAMESPACE: &str = "multi_agent_v1";
const MULTI_AGENT_V2_NAMESPACE: &str = "collaboration";
const SPAWN_AGENT_TOOL_NAME: &str = "spawn_agent";

fn init_git_repo(repo_path: &Path) -> Result<()> {
    run_git(repo_path, &["init"])?;
    run_git(repo_path, &["checkout", "-B", "main"])?;
    run_git(repo_path, &["config", "core.autocrlf", "false"])?;
    std::fs::write(repo_path.join("README.md"), "hello\n")?;
    run_git(repo_path, &["add", "README.md"])?;
    run_git(
        repo_path,
        &[
            "-c",
            "user.name=Tester",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial commit",
        ],
    )?;
    Ok(())
}

fn create_linked_worktree_fixture() -> Result<(tempfile::TempDir, PathBuf)> {
    let fixture = tempfile::TempDir::new()?;
    let main_repo = fixture.path().join("main");
    let linked_worktree = fixture.path().join("linked");
    std::fs::create_dir(&main_repo)?;
    init_git_repo(&main_repo)?;
    run_git(
        &main_repo,
        &[
            "worktree",
            "add",
            "-b",
            "linked-worktree",
            linked_worktree
                .to_str()
                .context("linked worktree path should be UTF-8")?,
            "HEAD",
        ],
    )?;
    Ok((fixture, linked_worktree))
}

fn run_git(repo_path: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .current_dir(repo_path)
        .args(args)
        .output()
        .with_context(|| format!("run git {args:?} in {}", repo_path.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed in {}: stdout={} stderr={}",
            args,
            repo_path.display(),
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn run_git_for_stdout(repo_path: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(repo_path)
        .args(args)
        .output()
        .with_context(|| format!("run git {args:?} in {}", repo_path.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed in {}: stdout={} stderr={}",
            args,
            repo_path.display(),
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn commit_tracked_subdir(repo_path: &Path, subdir: &str) -> Result<PathBuf> {
    let subdir_path = repo_path.join(subdir);
    std::fs::create_dir(&subdir_path)?;
    std::fs::write(subdir_path.join("README.md"), "subdir\n")?;
    run_git(repo_path, &["add", subdir])?;
    run_git(
        repo_path,
        &[
            "-c",
            "user.name=Tester",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "add subdir",
        ],
    )?;
    Ok(subdir_path)
}

fn function_call(call_id: &str, tool_name: &str, args: Value) -> Result<Value> {
    Ok(ev_function_call(
        call_id,
        tool_name,
        &serde_json::to_string(&args)?,
    ))
}

fn exec_stdout(output: &str) -> Result<String> {
    let (_, stdout) = output
        .split_once("Output:\n")
        .context("exec_command output should include an Output section")?;
    Ok(stdout.trim().to_string())
}

fn tool_names(body: &Value) -> Vec<String> {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| {
                    tool.get("name")
                        .or_else(|| tool.get("type"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn body_contains(request: &wiremock::Request, text: &str) -> bool {
    serde_json::from_slice::<Value>(&request.body).is_ok_and(|body| body.to_string().contains(text))
}

fn has_function_call_output(request: &wiremock::Request, call_id: &str) -> bool {
    serde_json::from_slice::<Value>(&request.body).is_ok_and(|body| {
        body.get("input")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("type").and_then(Value::as_str) == Some("function_call_output")
                        && item.get("call_id").and_then(Value::as_str) == Some(call_id)
                })
            })
    })
}
