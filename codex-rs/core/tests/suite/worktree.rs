use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use anyhow::Context;
use anyhow::Result;
use codex_features::Feature;
use codex_protocol::models::PermissionProfile;
use core_test_support::PathBufExt;
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
use pretty_assertions::assert_eq;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_worktree_without_args_returns_error_and_preserves_cwd() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree requires a local primary environment"
    );

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::UnifiedExec)
            .expect("test config should enable unified exec");
    });
    let test = builder.build(&server).await?;
    init_git_repo(test.config.cwd.as_path())?;

    let enter_call_id = "enter-worktree-without-args";
    let pwd_call_id = "pwd-after-rejected-enter";
    let pwd_args = json!({
        "cmd": "pwd",
        "yield_time_ms": 1_000_u64,
        "max_output_tokens": 2_000_u64,
    });
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(enter_call_id, ENTER_WORKTREE_TOOL_NAME, json!({}))?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_response_created("resp-2"),
            function_call(pwd_call_id, "exec_command", pwd_args)?,
            ev_completed("resp-2"),
        ]),
        sse(vec![
            ev_assistant_message("msg-3", "done"),
            ev_completed("resp-3"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "try entering a worktree without args, then check pwd",
        PermissionProfile::Disabled,
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    assert!(
        enter_output.contains("enter_worktree requires either `name` or `path`"),
        "unexpected enter_worktree output: {enter_output}"
    );

    let pwd = exec_stdout(
        &request_log
            .function_call_output_text(pwd_call_id)
            .context("missing pwd output")?,
    )?;
    assert_eq!(
        Path::new(&pwd).canonicalize()?,
        test.config.cwd.as_path().canonicalize()?
    );

    let common_dir = run_git_for_stdout(
        test.config.cwd.as_path(),
        &["rev-parse", "--git-common-dir"],
    )?;
    assert!(
        !test
            .config
            .cwd
            .as_path()
            .join(common_dir)
            .join("codex/worktrees")
            .exists(),
        "rejected enter_worktree call should not create managed worktrees"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_tools_retarget_later_cwd_sensitive_tools() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::UnifiedExec)
            .expect("test config should enable unified exec");
    });
    let test = builder.build(&server).await?;
    init_git_repo(test.config.cwd.as_path())?;

    let enter_call_id = "enter-worktree";
    let worktree_pwd_call_id = "repo-root-in-worktree";
    let exit_call_id = "exit-worktree";
    let original_pwd_call_id = "repo-root-after-exit";
    let exec_args = json!({
        "cmd": "git rev-parse --show-toplevel",
        "yield_time_ms": 1_000_u64,
        "max_output_tokens": 2_000_u64,
    });
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-sequencing" }),
            )?,
            function_call(worktree_pwd_call_id, "exec_command", exec_args.clone())?,
            function_call(exit_call_id, EXIT_WORKTREE_TOOL_NAME, json!({}))?,
            function_call(original_pwd_call_id, "exec_command", exec_args)?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "enter a worktree, check pwd, exit it, and check pwd",
        PermissionProfile::Disabled,
    )
    .await?;

    let requests = request_log.requests();
    let advertised_tools = tool_names(&requests[0].body_json());
    assert!(
        advertised_tools.contains(&ENTER_WORKTREE_TOOL_NAME.to_string()),
        "{ENTER_WORKTREE_TOOL_NAME} should be advertised; got {advertised_tools:?}"
    );
    assert!(
        advertised_tools.contains(&EXIT_WORKTREE_TOOL_NAME.to_string()),
        "{EXIT_WORKTREE_TOOL_NAME} should be advertised; got {advertised_tools:?}"
    );

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value = serde_json::from_str(&enter_output)
        .with_context(|| format!("enter_worktree output should be JSON: {enter_output}"))?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;
    assert_eq!(enter_output.get("cwd"), Some(&json!(worktree_path)));
    let common_dir = run_git_for_stdout(
        test.config.cwd.as_path(),
        &["rev-parse", "--git-common-dir"],
    )?;
    assert_eq!(
        Path::new(worktree_path).canonicalize()?,
        test.config
            .cwd
            .as_path()
            .join(common_dir.trim())
            .join("codex/worktrees/codex-sequencing")
            .canonicalize()?
    );

    let worktree_pwd = exec_stdout(
        &request_log
            .function_call_output_text(worktree_pwd_call_id)
            .context("missing worktree pwd output")?,
    )?;
    assert_eq!(
        Path::new(&worktree_pwd).canonicalize()?,
        Path::new(worktree_path).canonicalize()?
    );

    let exit_output = request_log
        .function_call_output_text(exit_call_id)
        .context("missing exit_worktree output")?;
    let exit_output: Value =
        serde_json::from_str(&exit_output).context("exit_worktree output should be JSON")?;
    assert_eq!(
        exit_output.get("cwd"),
        Some(&json!(test.config.cwd.to_string_lossy().to_string()))
    );
    assert_eq!(exit_output.get("keep"), Some(&json!(true)));
    assert_eq!(exit_output.get("removed"), Some(&json!(false)));
    assert!(
        Path::new(worktree_path).exists(),
        "default exit_worktree should keep the managed worktree"
    );

    let original_pwd = exec_stdout(
        &request_log
            .function_call_output_text(original_pwd_call_id)
            .context("missing original pwd output")?,
    )?;
    assert_eq!(
        Path::new(&original_pwd).canonicalize()?,
        test.config.cwd.as_path().canonicalize()?
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exit_worktree_keep_true_keeps_managed_worktree() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let server = start_mock_server().await;
    let test = test_codex().build(&server).await?;
    init_git_repo(test.config.cwd.as_path())?;

    let enter_call_id = "enter-worktree";
    let exit_call_id = "exit-worktree";
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-keep-true" }),
            )?,
            function_call(
                exit_call_id,
                EXIT_WORKTREE_TOOL_NAME,
                json!({ "keep": true }),
            )?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "enter a worktree and exit while keeping it",
        PermissionProfile::workspace_write(),
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value =
        serde_json::from_str(&enter_output).context("enter_worktree output should be JSON")?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;
    assert!(
        Path::new(worktree_path).exists(),
        "keep=true should leave managed worktree on disk"
    );

    let exit_output = request_log
        .function_call_output_text(exit_call_id)
        .context("missing exit_worktree output")?;
    let exit_output: Value =
        serde_json::from_str(&exit_output).context("exit_worktree output should be JSON")?;
    assert_eq!(
        exit_output.get("cwd"),
        Some(&json!(test.config.cwd.to_string_lossy().to_string()))
    );
    assert_eq!(exit_output.get("keep"), Some(&json!(true)));
    assert_eq!(exit_output.get("removed"), Some(&json!(false)));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exit_worktree_keep_false_removes_clean_managed_worktree() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let server = start_mock_server().await;
    let test = test_codex().build(&server).await?;
    init_git_repo(test.config.cwd.as_path())?;

    let enter_call_id = "enter-worktree";
    let exit_call_id = "exit-worktree";
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-remove-clean" }),
            )?,
            function_call(
                exit_call_id,
                EXIT_WORKTREE_TOOL_NAME,
                json!({ "keep": false }),
            )?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "enter a worktree and exit while removing it",
        PermissionProfile::workspace_write(),
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value =
        serde_json::from_str(&enter_output).context("enter_worktree output should be JSON")?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;
    assert!(
        !Path::new(worktree_path).exists(),
        "keep=false should remove clean managed worktree from disk"
    );
    let worktree_list = run_git_for_stdout(test.config.cwd.as_path(), &["worktree", "list"])?;
    assert!(
        !worktree_list.contains(worktree_path),
        "removed worktree should not appear in git worktree list"
    );

    let exit_output = request_log
        .function_call_output_text(exit_call_id)
        .context("missing exit_worktree output")?;
    let exit_output: Value =
        serde_json::from_str(&exit_output).context("exit_worktree output should be JSON")?;
    assert_eq!(
        exit_output.get("cwd"),
        Some(&json!(test.config.cwd.to_string_lossy().to_string()))
    );
    assert_eq!(exit_output.get("keep"), Some(&json!(false)));
    assert_eq!(exit_output.get("removed"), Some(&json!(true)));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exit_worktree_keep_false_rejects_dirty_managed_worktree() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::UnifiedExec)
            .expect("test config should enable unified exec");
    });
    let test = builder.build(&server).await?;
    init_git_repo(test.config.cwd.as_path())?;

    let enter_call_id = "enter-worktree";
    let dirty_call_id = "dirty-worktree";
    let exit_call_id = "exit-worktree";
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-remove-dirty" }),
            )?,
            function_call(
                dirty_call_id,
                "exec_command",
                json!({
                    "cmd": "printf dirty > dirty.txt",
                    "yield_time_ms": 1_000_u64,
                    "max_output_tokens": 2_000_u64,
                }),
            )?,
            function_call(
                exit_call_id,
                EXIT_WORKTREE_TOOL_NAME,
                json!({ "keep": false }),
            )?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "enter a worktree, dirty it, and try to remove it while exiting",
        PermissionProfile::Disabled,
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value =
        serde_json::from_str(&enter_output).context("enter_worktree output should be JSON")?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;
    assert!(
        Path::new(worktree_path).exists(),
        "dirty managed worktree should be left in place"
    );

    let exit_output = request_log
        .function_call_output_text(exit_call_id)
        .context("missing exit_worktree output")?;
    assert!(
        exit_output.contains("worktree operation failed")
            && exit_output.contains("contains modified or untracked files"),
        "dirty removal should report git's refusal, got {exit_output:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exit_worktree_rejects_unknown_args() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(Ok(()), "exit_worktree requires a local primary environment");

    let server = start_mock_server().await;
    let test = test_codex().build(&server).await?;

    let exit_call_id = "exit-worktree";
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                exit_call_id,
                EXIT_WORKTREE_TOOL_NAME,
                json!({ "remove": true }),
            )?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "call exit_worktree with the wrong argument",
        PermissionProfile::Disabled,
    )
    .await?;

    let exit_output = request_log
        .function_call_output_text(exit_call_id)
        .context("missing exit_worktree output")?;
    assert!(
        exit_output.contains("unknown field") && exit_output.contains("remove"),
        "unknown argument should be rejected, got {exit_output:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_flow_isolates_patch_from_original_checkout() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::UnifiedExec)
            .expect("test config should enable unified exec");
    });
    let test = builder.build(&server).await?;
    init_git_repo(test.config.cwd.as_path())?;

    let marker_file = "DOGFOOD_WORKTREE_MARKER.txt";
    let marker_contents = "created inside managed worktree\n";
    let patch =
        format!("*** Begin Patch\n*** Add File: {marker_file}\n+{marker_contents}*** End Patch\n");
    let enter_call_id = "enter-worktree";
    let patch_call_id = "patch-worktree-marker";
    let worktree_status_call_id = "worktree-status";
    let exit_call_id = "exit-worktree";
    let original_status_call_id = "original-status";
    let status_args = json!({
        "cmd": format!("git status --short {marker_file}"),
        "yield_time_ms": 1_000_u64,
        "max_output_tokens": 2_000_u64,
    });
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-dogfood-flow" }),
            )?,
            ev_apply_patch_custom_tool_call(patch_call_id, &patch),
            function_call(worktree_status_call_id, "exec_command", status_args.clone())?,
            function_call(exit_call_id, EXIT_WORKTREE_TOOL_NAME, json!({}))?,
            function_call(original_status_call_id, "exec_command", status_args)?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "enter a worktree, make an isolated patch, exit it, and compare status",
        PermissionProfile::Disabled,
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value = serde_json::from_str(&enter_output)
        .with_context(|| format!("enter_worktree output should be JSON: {enter_output}"))?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;
    let worktree_marker = Path::new(worktree_path).join(marker_file);
    assert_eq!(
        std::fs::read_to_string(&worktree_marker)
            .with_context(|| format!("read {}", worktree_marker.display()))?,
        marker_contents
    );
    assert!(
        !test.config.cwd.as_path().join(marker_file).exists(),
        "marker file should not be created in the original checkout"
    );

    let worktree_status = exec_stdout(
        &request_log
            .function_call_output_text(worktree_status_call_id)
            .context("missing worktree status output")?,
    )?;
    assert_eq!(worktree_status, format!("?? {marker_file}"));

    let exit_output = request_log
        .function_call_output_text(exit_call_id)
        .context("missing exit_worktree output")?;
    let exit_output: Value =
        serde_json::from_str(&exit_output).context("exit_worktree output should be JSON")?;
    assert_eq!(
        exit_output.get("cwd"),
        Some(&json!(test.config.cwd.to_string_lossy().to_string()))
    );

    let original_status = exec_stdout(
        &request_log
            .function_call_output_text(original_status_call_id)
            .context("missing original status output")?,
    )?;
    assert_eq!(original_status, "");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_worktree_allows_linked_worktree_with_external_common_git_dir() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let (_fixture, linked_worktree) = create_linked_worktree_fixture()?;
    let linked_worktree = dunce::canonicalize(linked_worktree)?.abs();
    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(move |config| {
        config.cwd = linked_worktree;
        config.workspace_roots = vec![config.cwd.clone()];
    });
    let test = builder.build(&server).await?;

    let enter_call_id = "enter-linked-worktree";
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-linked-flow" }),
            )?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "enter a managed worktree from a linked worktree",
        PermissionProfile::workspace_write(),
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value = serde_json::from_str(&enter_output)
        .with_context(|| format!("enter_worktree output should be JSON: {enter_output}"))?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;
    let worktree_path = Path::new(worktree_path)
        .canonicalize()
        .with_context(|| format!("canonicalize entered worktree path `{worktree_path}`"))?;
    assert!(worktree_path.is_dir());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_worktree_allows_writable_repo_subdirectory_cwd() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let fixture = tempfile::TempDir::new()?;
    let repo = fixture.path().join("repo");
    let subdir = repo.join("codex-rs");
    std::fs::create_dir(&repo)?;
    init_git_repo(&repo)?;
    std::fs::create_dir(&subdir)?;

    let subdir = dunce::canonicalize(subdir)?.abs();
    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(move |config| {
        config.cwd = subdir;
        config.workspace_roots = vec![config.cwd.clone()];
    });
    let test = builder.build(&server).await?;

    let enter_call_id = "enter-subdir-worktree";
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-subdir-flow" }),
            )?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "enter a managed worktree from a writable repo subdirectory",
        PermissionProfile::workspace_write(),
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value = serde_json::from_str(&enter_output)
        .with_context(|| format!("enter_worktree output should be JSON: {enter_output}"))?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;
    let worktree_path = Path::new(worktree_path)
        .canonicalize()
        .with_context(|| format!("canonicalize entered worktree path `{worktree_path}`"))?;
    assert!(worktree_path.is_dir());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_worktree_preserves_relative_subdirectory_cwd() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let fixture = tempfile::TempDir::new()?;
    let repo = fixture.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_git_repo(&repo)?;
    let subdir = commit_tracked_subdir(&repo, "codex-rs")?;

    let subdir = dunce::canonicalize(subdir)?.abs();
    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(move |config| {
        config.cwd = subdir;
        config.workspace_roots = vec![config.cwd.clone()];
        config
            .features
            .enable(Feature::UnifiedExec)
            .expect("test config should enable unified exec");
    });
    let test = builder.build(&server).await?;

    let enter_call_id = "enter-subdir-worktree";
    let pwd_call_id = "pwd-in-subdir-worktree";
    let exit_call_id = "exit-subdir-worktree";
    let original_pwd_call_id = "pwd-after-subdir-exit";
    let pwd_args = json!({
        "cmd": "pwd",
        "yield_time_ms": 1_000_u64,
        "max_output_tokens": 2_000_u64,
    });
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-subdir-cwd" }),
            )?,
            function_call(pwd_call_id, "exec_command", pwd_args.clone())?,
            function_call(exit_call_id, EXIT_WORKTREE_TOOL_NAME, json!({}))?,
            function_call(original_pwd_call_id, "exec_command", pwd_args)?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "enter a managed worktree from a subdirectory, check pwd, exit it, and check pwd",
        PermissionProfile::Disabled,
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value = serde_json::from_str(&enter_output)
        .with_context(|| format!("enter_worktree output should be JSON: {enter_output}"))?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;
    let expected_cwd = Path::new(worktree_path)
        .join("codex-rs")
        .canonicalize()
        .with_context(|| {
            format!("canonicalize expected worktree subdir under `{worktree_path}`")
        })?;
    assert_eq!(
        enter_output.get("cwd"),
        Some(&json!(expected_cwd.to_string_lossy().to_string()))
    );

    let worktree_pwd = exec_stdout(
        &request_log
            .function_call_output_text(pwd_call_id)
            .context("missing worktree pwd output")?,
    )?;
    let worktree_pwd = Path::new(&worktree_pwd)
        .canonicalize()
        .with_context(|| format!("canonicalize worktree pwd output `{worktree_pwd}`"))?;
    assert_eq!(worktree_pwd, expected_cwd);

    let exit_output = request_log
        .function_call_output_text(exit_call_id)
        .context("missing exit_worktree output")?;
    let exit_output: Value =
        serde_json::from_str(&exit_output).context("exit_worktree output should be JSON")?;
    assert_eq!(
        exit_output.get("cwd"),
        Some(&json!(test.config.cwd.to_string_lossy().to_string()))
    );

    let original_pwd = exec_stdout(
        &request_log
            .function_call_output_text(original_pwd_call_id)
            .context("missing original pwd output")?,
    )?;
    assert_eq!(
        Path::new(&original_pwd)
            .canonicalize()
            .with_context(|| format!("canonicalize original pwd output `{original_pwd}`"))?,
        test.config.cwd.as_path().canonicalize()?
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_existing_worktree_preserves_relative_subdirectory_cwd() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let fixture = tempfile::TempDir::new()?;
    let repo = fixture.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_git_repo(&repo)?;
    let subdir = commit_tracked_subdir(&repo, "codex-rs")?;
    let managed =
        codex_git_utils::create_or_reuse_managed_worktree(&repo, "codex-existing-subdir")?;

    let subdir = dunce::canonicalize(subdir)?.abs();
    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(move |config| {
        config.cwd = subdir;
        config.workspace_roots = vec![config.cwd.clone()];
        config
            .features
            .enable(Feature::UnifiedExec)
            .expect("test config should enable unified exec");
    });
    let test = builder.build(&server).await?;

    let enter_call_id = "enter-existing-subdir-worktree";
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "path": managed.path }),
            )?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "enter an existing managed worktree from a subdirectory and check pwd",
        PermissionProfile::workspace_write(),
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value = serde_json::from_str(&enter_output)
        .with_context(|| format!("enter_worktree output should be JSON: {enter_output}"))?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;
    let expected_cwd = Path::new(worktree_path)
        .join("codex-rs")
        .canonicalize()
        .with_context(|| {
            format!("canonicalize expected worktree subdir under `{worktree_path}`")
        })?;
    assert_eq!(
        enter_output.get("cwd"),
        Some(&json!(expected_cwd.to_string_lossy().to_string()))
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_existing_worktree_ignores_relative_subdirectory_symlink_escape() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree and exit_worktree require a local primary environment"
    );

    let fixture = tempfile::TempDir::new()?;
    let repo = fixture.path().join("repo");
    let outside = fixture.path().join("outside");
    std::fs::create_dir(&repo)?;
    std::fs::create_dir(&outside)?;
    init_git_repo(&repo)?;
    let subdir = commit_tracked_subdir(&repo, "codex-rs")?;
    let managed = codex_git_utils::create_or_reuse_managed_worktree(&repo, "codex-symlink-subdir")?;
    std::fs::remove_dir_all(managed.path.join("codex-rs"))?;
    symlink_dir(&outside, managed.path.join("codex-rs"))?;

    let subdir = dunce::canonicalize(subdir)?.abs();
    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(move |config| {
        config.cwd = subdir;
        config.workspace_roots = vec![config.cwd.clone()];
    });
    let test = builder.build(&server).await?;

    let enter_call_id = "enter-symlink-subdir-worktree";
    let responses = vec![
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "path": managed.path }),
            )?,
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    ];
    let request_log = mount_sse_sequence(&server, responses).await;

    test.submit_turn_with_permission_profile(
        "enter an existing managed worktree whose matching subdirectory escapes via symlink",
        PermissionProfile::workspace_write(),
    )
    .await?;

    let enter_output = request_log
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value = serde_json::from_str(&enter_output)
        .with_context(|| format!("enter_worktree output should be JSON: {enter_output}"))?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;
    assert_eq!(enter_output.get("cwd"), Some(&json!(worktree_path)));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_worktree_retargets_same_turn_spawned_agent_cwd() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree requires a local primary environment"
    );

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::Collab)
            .expect("test config should enable collab");
    });
    let test = builder.build(&server).await?;
    init_git_repo(test.config.cwd.as_path())?;

    let prompt = "enter a worktree and spawn an agent there";
    let enter_call_id = "enter-worktree";
    let spawn_call_id = "spawn-worker";
    let child_prompt = "report your cwd";
    let spawn_args = serde_json::to_string(&json!({
        "message": child_prompt,
    }))?;
    mount_sse_once_match(
        &server,
        move |request: &wiremock::Request| body_contains(request, prompt),
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-spawn" }),
            )?,
            ev_function_call_with_namespace(
                spawn_call_id,
                MULTI_AGENT_V1_NAMESPACE,
                SPAWN_AGENT_TOOL_NAME,
                &spawn_args,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;
    let child_turn = mount_sse_once_match(
        &server,
        move |request: &wiremock::Request| {
            body_contains(request, child_prompt)
                && !body_contains(request, spawn_call_id)
                && !has_function_call_output(request, enter_call_id)
                && !has_function_call_output(request, spawn_call_id)
        },
        sse(vec![
            ev_response_created("resp-child"),
            ev_completed("resp-child"),
        ]),
    )
    .await;
    let parent_followup = mount_sse_once_match(
        &server,
        move |request: &wiremock::Request| has_function_call_output(request, spawn_call_id),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    test.submit_turn_with_permission_profile(prompt, PermissionProfile::Disabled)
        .await?;

    let enter_output = parent_followup
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value =
        serde_json::from_str(&enter_output).context("enter_worktree output should be JSON")?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;

    let child_requests = child_turn.requests();
    assert!(
        child_requests
            .iter()
            .any(|request| request.body_contains_text(worktree_path)),
        "child request should use worktree cwd {worktree_path}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enter_worktree_retargets_same_turn_v2_spawned_agent_cwd() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "enter_worktree requires a local primary environment"
    );

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::Collab)
            .expect("test config should enable collab");
        config
            .features
            .enable(Feature::MultiAgentV2)
            .expect("test config should enable multi-agent v2");
    });
    let test = builder.build(&server).await?;
    init_git_repo(test.config.cwd.as_path())?;

    let prompt = "enter a worktree and spawn a v2 agent there";
    let enter_call_id = "enter-worktree";
    let spawn_call_id = "spawn-worker-v2";
    let child_prompt = "report your v2 cwd";
    let spawn_args = serde_json::to_string(&json!({
        "message": child_prompt,
        "task_name": "worker",
    }))?;
    mount_sse_once_match(
        &server,
        move |request: &wiremock::Request| body_contains(request, prompt),
        sse(vec![
            ev_response_created("resp-1"),
            function_call(
                enter_call_id,
                ENTER_WORKTREE_TOOL_NAME,
                json!({ "name": "codex-spawn-v2" }),
            )?,
            ev_function_call_with_namespace(
                spawn_call_id,
                MULTI_AGENT_V2_NAMESPACE,
                SPAWN_AGENT_TOOL_NAME,
                &spawn_args,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;
    let child_turn = mount_sse_once_match(
        &server,
        move |request: &wiremock::Request| {
            body_contains(request, child_prompt)
                && !body_contains(request, spawn_call_id)
                && !has_function_call_output(request, enter_call_id)
                && !has_function_call_output(request, spawn_call_id)
        },
        sse(vec![
            ev_response_created("resp-child"),
            ev_completed("resp-child"),
        ]),
    )
    .await;
    let parent_followup = mount_sse_once_match(
        &server,
        move |request: &wiremock::Request| has_function_call_output(request, spawn_call_id),
        sse(vec![
            ev_assistant_message("msg-2", "done"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    test.submit_turn_with_permission_profile(prompt, PermissionProfile::Disabled)
        .await?;

    let enter_output = parent_followup
        .function_call_output_text(enter_call_id)
        .context("missing enter_worktree output")?;
    let enter_output: Value =
        serde_json::from_str(&enter_output).context("enter_worktree output should be JSON")?;
    let worktree_path = enter_output
        .get("worktree_path")
        .and_then(Value::as_str)
        .context("enter_worktree output should include worktree_path")?;

    let child_requests = child_turn.requests();
    assert!(
        child_requests
            .iter()
            .any(|request| request.body_contains_text(worktree_path)),
        "v2 child request should use worktree cwd {worktree_path}"
    );

    Ok(())
}
