use super::*;
use crate::legacy_core::config::ConfigBuilder;
use codex_app_server_client::AppServerRequestHandle;
use codex_app_server_client::DEFAULT_IN_PROCESS_CHANNEL_CAPACITY;
use codex_app_server_client::InProcessAppServerClient;
use codex_app_server_client::InProcessClientStartArgs;
use codex_app_server_client::TypedRequestError;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::CommandExecParams;
use codex_app_server_protocol::CommandExecResponse;
use codex_app_server_protocol::CommandExecWriteParams;
use codex_app_server_protocol::CommandExecWriteResponse;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::RequestId;
use codex_arg0::Arg0DispatchPaths;
use codex_cloud_config::cloud_config_bundle_loader_for_storage;
use codex_config::types::AuthCredentialsStoreMode;
use codex_feedback::CodexFeedback;
use codex_login::AuthKeyringBackendKind;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn app_server_runner_streams_stdin_and_returns_capped_buffered_output() -> anyhow::Result<()>
{
    let client = start_test_app_server_client().await?;
    let runner = AppServerWorkspaceCommandRunner::new(
        AppServerRequestHandle::InProcess(client.client.request_handle()),
        Some("linux"),
    );
    let cwd = TempDir::new()?;

    let output = runner
        .run(
            WorkspaceCommand::new([
                "sh",
                "-lc",
                "IFS= read line; printf 'out:%s\\n' \"$line\"; printf 'err:%s\\n' \"$line\" >&2",
            ])
            .cwd(cwd.path())
            .output_bytes_cap(5)
            .stdin("hello\n"),
        )
        .await?;

    assert_eq!(
        output,
        WorkspaceCommandOutput {
            exit_code: 0,
            stdout: "out:h".to_string(),
            stderr: "err:h".to_string(),
        }
    );
    Ok(())
}

#[tokio::test]
async fn rejected_stdin_write_can_terminate_active_command() -> anyhow::Result<()> {
    let client = start_test_app_server_client().await?;
    let request_handle = AppServerRequestHandle::InProcess(client.client.request_handle());
    let cwd = TempDir::new()?;
    let process_id = "workspace-command-test-no-stdin".to_string();
    let exec_request_handle = request_handle.clone();
    let exec_process_id = process_id.clone();
    let exec_task = tokio::spawn(async move {
        exec_request_handle
            .request_typed::<CommandExecResponse>(ClientRequest::OneOffCommandExec {
                request_id: RequestId::String("workspace-command-test-exec".to_string()),
                params: CommandExecParams {
                    command: vec!["sh".to_string(), "-lc".to_string(), "sleep 10".to_string()],
                    process_id: Some(exec_process_id),
                    tty: false,
                    stream_stdin: false,
                    stream_stdout_stderr: false,
                    output_bytes_cap: Some(1024),
                    disable_output_cap: false,
                    disable_timeout: false,
                    timeout_ms: Some(10_000),
                    cwd: Some(cwd.path().to_path_buf()),
                    env: None,
                    size: None,
                    sandbox_policy: None,
                    permission_profile: None,
                },
            })
            .await
    });

    let err =
        write_workspace_command_stdin(&request_handle, process_id.as_str(), b"hello".to_vec())
            .await
            .expect_err("write should fail when stdin streaming is disabled");
    assert!(
        err.to_string()
            .contains("stdin streaming is not enabled for this command/exec")
    );

    terminate_workspace_command(&request_handle, process_id.as_str()).await?;
    let _ = exec_task.await??;
    Ok(())
}

#[tokio::test]
async fn missing_command_exec_write_returns_stable_retry_reason() -> anyhow::Result<()> {
    let client = start_test_app_server_client().await?;
    let request_handle = AppServerRequestHandle::InProcess(client.client.request_handle());

    let err = request_handle
        .request_typed::<CommandExecWriteResponse>(ClientRequest::CommandExecWrite {
            request_id: RequestId::String("workspace-command-test-missing-write".to_string()),
            params: CommandExecWriteParams {
                process_id: "missing".to_string(),
                delta_base64: Some("aGVsbG8=".to_string()),
                close_stdin: true,
            },
        })
        .await
        .expect_err("missing command/exec should fail");

    assert!(command_exec_write_error_retryable(&err));
    let TypedRequestError::Server { source, .. } = err else {
        panic!("expected server error");
    };
    assert_eq!(
        source.data.and_then(|data| data.get("reason").cloned()),
        Some(serde_json::json!("command_exec_not_active"))
    );
    Ok(())
}

#[test]
fn command_exec_write_retryable_uses_stable_reason() {
    let err = TypedRequestError::Server {
        method: "command/exec/write".to_string(),
        source: JSONRPCErrorError {
            code: -32600,
            message: "renamed server message".to_string(),
            data: Some(serde_json::json!({
                "reason": "command_exec_not_active",
            })),
        },
    };

    assert!(command_exec_write_error_retryable(&err));
}

#[test]
fn command_exec_write_retryable_ignores_matching_prose_without_reason() {
    let err = TypedRequestError::Server {
        method: "command/exec/write".to_string(),
        source: JSONRPCErrorError {
            code: -32600,
            message: "no active command/exec for process id `demo`".to_string(),
            data: None,
        },
    };

    assert!(!command_exec_write_error_retryable(&err));
}

struct TestAppServerClient {
    _codex_home: TempDir,
    client: InProcessAppServerClient,
}

async fn start_test_app_server_client() -> anyhow::Result<TestAppServerClient> {
    let codex_home = TempDir::new()?;
    let codex_home_path = codex_home.path().to_path_buf();
    let config = ConfigBuilder::default()
        .codex_home(codex_home_path.clone())
        .build()
        .await?;
    let client = InProcessAppServerClient::start(InProcessClientStartArgs {
        arg0_paths: Arg0DispatchPaths::default(),
        config: Arc::new(config),
        cli_overrides: Vec::new(),
        loader_overrides: Default::default(),
        strict_config: false,
        cloud_config_bundle: cloud_config_bundle_loader_for_storage(
            codex_home_path,
            /*enable_codex_api_key_env*/ false,
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
            "https://chatgpt.com/backend-api/".to_string(),
            /*auth_route_config*/ None,
        )
        .await,
        feedback: CodexFeedback::new(),
        log_db: None,
        state_db: None,
        environment_manager: Arc::new(
            codex_app_server_client::EnvironmentManager::default_for_tests(),
        ),
        config_warnings: Vec::new(),
        session_source: serde_json::from_value(serde_json::json!("cli"))?,
        enable_codex_api_key_env: false,
        client_name: "test".to_string(),
        client_version: "test".to_string(),
        experimental_api: true,
        mcp_server_openai_form_elicitation: false,
        opt_out_notification_methods: Vec::new(),
        channel_capacity: DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
    })
    .await?;
    Ok(TestAppServerClient {
        _codex_home: codex_home,
        client,
    })
}
