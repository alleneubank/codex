//! App-server-backed workspace command execution for TUI-owned background lookups.
//!
//! This module is the TUI boundary for non-interactive commands that need to run wherever
//! the active workspace lives. Callers describe a command in terms of argv, cwd, environment
//! overrides, stdin, timeout, output cap, and permission profile; the runner translates that
//! request to app-server `command/exec` plus `command/exec/write` when stdin is present. Keeping
//! this as a TUI-local abstraction lets status surfaces avoid knowing whether the current
//! app-server is embedded or remote.
//!
//! Most callers should keep output bounded so metadata refreshes cannot grow into unbounded
//! background processes; callers that own a full user-visible payload, such as `/diff`, can
//! explicitly opt out of output capping.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use codex_app_server_client::AppServerRequestHandle;
use codex_app_server_client::TypedRequestError;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::CommandExecParams;
use codex_app_server_protocol::CommandExecResponse;
use codex_app_server_protocol::CommandExecTerminateParams;
use codex_app_server_protocol::CommandExecTerminateResponse;
use codex_app_server_protocol::CommandExecWriteParams;
use codex_app_server_protocol::CommandExecWriteResponse;
use codex_app_server_protocol::RequestId;
use uuid::Uuid;

const STDIN_WRITE_FAILURE_RESULT_GRACE: Duration = Duration::from_millis(50);
const STDIN_WRITE_MAX_ATTEMPTS: usize = 500;
const STDIN_WRITE_RETRY_DELAY: Duration = Duration::from_millis(10);

/// Shared handle for running workspace commands from TUI components.
pub(crate) type WorkspaceCommandRunner = Arc<dyn WorkspaceCommandExecutor>;

/// Describes a bounded non-interactive command to execute in the active workspace.
///
/// The command is intentionally argv-based rather than shell-based so callers do not need to quote
/// user or repository data. `cwd` is interpreted by app-server relative to the workspace rules for
/// the active session, which is what makes the same request shape work for embedded and remote
/// app-server instances.
#[derive(Clone, Debug)]
pub(crate) struct WorkspaceCommand {
    /// Program and arguments to execute without shell interpolation.
    pub(crate) argv: Vec<String>,
    /// Working directory for the command, if different from app-server's session cwd.
    pub(crate) cwd: Option<PathBuf>,
    /// Environment overrides where `None` removes a variable.
    pub(crate) env: HashMap<String, Option<String>>,
    /// Maximum wall-clock duration before app-server cancels the command.
    pub(crate) timeout: Duration,
    /// Output cap behavior for captured stdout/stderr returned by app-server.
    pub(crate) output_cap: WorkspaceCommandOutputCap,
    /// Optional bytes to write to stdin before closing it.
    pub(crate) stdin: Option<Vec<u8>>,
    /// Active app-server permission profile id to use for this command.
    pub(crate) permission_profile: Option<String>,
}

impl WorkspaceCommand {
    /// Creates a workspace command with conservative defaults for metadata probes.
    pub(crate) fn new(argv: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            argv: argv.into_iter().map(Into::into).collect(),
            cwd: None,
            env: HashMap::new(),
            timeout: Duration::from_secs(/*secs*/ 5),
            output_cap: WorkspaceCommandOutputCap::Bytes(64 * 1024),
            stdin: None,
            permission_profile: None,
        }
    }

    /// Sets the command working directory.
    pub(crate) fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Adds or replaces one environment variable override.
    pub(crate) fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), Some(value.into()));
        self
    }

    /// Sets the maximum wall-clock duration before app-server cancels the command.
    pub(crate) fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the per-stream stdout/stderr cap returned by app-server.
    pub(crate) fn output_bytes_cap(mut self, output_bytes_cap: usize) -> Self {
        self.output_cap = WorkspaceCommandOutputCap::Bytes(output_bytes_cap);
        self
    }

    /// Uses app-server's default output cap.
    pub(crate) fn use_default_output_cap(mut self) -> Self {
        self.output_cap = WorkspaceCommandOutputCap::Default;
        self
    }

    /// Requests uncapped stdout/stderr capture from app-server.
    pub(crate) fn disable_output_cap(mut self) -> Self {
        self.output_cap = WorkspaceCommandOutputCap::Disabled;
        self
    }

    /// Writes bytes to stdin and then closes it after the command starts.
    pub(crate) fn stdin(mut self, stdin: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(stdin.into());
        self
    }
}

/// Mutually exclusive output cap modes for workspace command output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkspaceCommandOutputCap {
    /// Send an explicit per-stream byte cap to app-server.
    Bytes(usize),
    /// Let app-server apply its default output cap.
    Default,
    /// Ask app-server to return uncapped output.
    Disabled,
}

/// Captured result from a completed workspace command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkspaceCommandOutput {
    /// Process exit status code reported by app-server.
    pub(crate) exit_code: i32,
    /// Captured stdout after app-server output capping.
    pub(crate) stdout: String,
    /// Captured stderr after app-server output capping.
    pub(crate) stderr: String,
}

impl WorkspaceCommandOutput {
    /// Returns whether the process exited successfully.
    pub(crate) fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Transport or protocol failure before a command result was available.
///
/// Non-zero process exits are represented as `WorkspaceCommandOutput` so callers can distinguish
/// a normal probe miss from an app-server request failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkspaceCommandError {
    message: String,
}

impl WorkspaceCommandError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for WorkspaceCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for WorkspaceCommandError {}

/// Executes non-interactive workspace commands through the active TUI app-server session.
///
/// Implementations decide where the workspace lives. Callers provide argv/cwd/env and should not
/// branch on local versus remote execution.
pub(crate) trait WorkspaceCommandExecutor: Send + Sync {
    /// Platform that executes app-server commands.
    fn platform(&self) -> WorkspaceCommandPlatform {
        WorkspaceCommandPlatform::current()
    }

    /// Runs a workspace command and returns captured output or an app-server request error.
    ///
    /// Callers should treat errors as infrastructure failures and should treat successful output
    /// with a non-zero exit code as ordinary command failure. Returning a boxed future keeps the
    /// trait object-safe.
    fn run(
        &self,
        command: WorkspaceCommand,
    ) -> Pin<
        Box<dyn Future<Output = Result<WorkspaceCommandOutput, WorkspaceCommandError>> + Send + '_>,
    >;
}

/// App-server command execution platform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkspaceCommandPlatform {
    Unknown,
    Unix,
    Windows,
}

impl WorkspaceCommandPlatform {
    pub(crate) fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }

    fn from_platform_os(platform_os: Option<&str>) -> Self {
        match platform_os {
            Some(platform_os) if platform_os.eq_ignore_ascii_case("windows") => Self::Windows,
            Some(platform_os)
                if platform_os.eq_ignore_ascii_case("linux")
                    || platform_os.eq_ignore_ascii_case("macos") =>
            {
                Self::Unix
            }
            Some(_) | None => Self::Unknown,
        }
    }

    pub(crate) fn is_windows(self) -> bool {
        matches!(self, Self::Windows)
    }
}

/// Workspace command runner that forwards every request to the active app-server.
#[derive(Clone)]
pub(crate) struct AppServerWorkspaceCommandRunner {
    request_handle: AppServerRequestHandle,
    platform: WorkspaceCommandPlatform,
}

impl AppServerWorkspaceCommandRunner {
    /// Creates a runner from an app-server request handle owned by the current TUI session.
    pub(crate) fn new(request_handle: AppServerRequestHandle, platform_os: Option<&str>) -> Self {
        Self {
            request_handle,
            platform: WorkspaceCommandPlatform::from_platform_os(platform_os),
        }
    }
}

impl WorkspaceCommandExecutor for AppServerWorkspaceCommandRunner {
    fn platform(&self) -> WorkspaceCommandPlatform {
        self.platform
    }

    /// Sends the command as a one-off app-server `command/exec` request, with optional stdin
    /// streamed through `command/exec/write`.
    ///
    /// The request is non-tty and uses the caller's timeout and output cap. It leaves sandbox and
    /// permission profile selection to app-server so the same runner follows the active session's
    /// embedded or remote execution policy.
    fn run(
        &self,
        command: WorkspaceCommand,
    ) -> Pin<
        Box<dyn Future<Output = Result<WorkspaceCommandOutput, WorkspaceCommandError>> + Send + '_>,
    > {
        Box::pin(async move {
            let WorkspaceCommand {
                argv,
                cwd,
                env,
                timeout,
                output_cap,
                stdin,
                permission_profile,
            } = command;
            let timeout_ms = i64::try_from(timeout.as_millis()).unwrap_or(i64::MAX);
            let env = if env.is_empty() { None } else { Some(env) };
            let (bounded_output_cap, disable_output_cap) = match output_cap {
                WorkspaceCommandOutputCap::Bytes(output_bytes_cap) => {
                    (Some(output_bytes_cap), false)
                }
                WorkspaceCommandOutputCap::Default => (None, false),
                WorkspaceCommandOutputCap::Disabled => (None, true),
            };

            let Some(stdin) = stdin else {
                let request = ClientRequest::OneOffCommandExec {
                    request_id: RequestId::String(format!("workspace-command-{}", Uuid::new_v4())),
                    params: CommandExecParams {
                        command: argv,
                        process_id: None,
                        tty: false,
                        stream_stdin: false,
                        stream_stdout_stderr: false,
                        output_bytes_cap: bounded_output_cap,
                        disable_output_cap,
                        disable_timeout: false,
                        timeout_ms: Some(timeout_ms),
                        cwd,
                        env,
                        size: None,
                        sandbox_policy: None,
                        permission_profile,
                    },
                };
                let response: CommandExecResponse = self
                    .request_handle
                    .request_typed(request)
                    .await
                    .map_err(|err| WorkspaceCommandError::new(err.to_string()))?;
                return Ok(response.into());
            };

            let request_handle = self.request_handle.clone();
            let exec_request_handle = self.request_handle.clone();
            let process_id = format!("workspace-command-{}", Uuid::new_v4());
            let request = ClientRequest::OneOffCommandExec {
                request_id: RequestId::String(format!("workspace-command-{}", Uuid::new_v4())),
                params: CommandExecParams {
                    command: argv,
                    process_id: Some(process_id.clone()),
                    tty: false,
                    stream_stdin: true,
                    stream_stdout_stderr: false,
                    output_bytes_cap: bounded_output_cap,
                    disable_output_cap,
                    disable_timeout: false,
                    timeout_ms: Some(timeout_ms),
                    cwd,
                    env,
                    size: None,
                    sandbox_policy: None,
                    permission_profile,
                },
            };
            let mut exec_task = tokio::spawn(async move {
                exec_request_handle
                    .request_typed::<CommandExecResponse>(request)
                    .await
            });

            let write_result = tokio::select! {
                response = &mut exec_task => {
                    let response = response
                        .map_err(|err| {
                            WorkspaceCommandError::new(format!("command task failed: {err}"))
                        })?
                        .map_err(|err| WorkspaceCommandError::new(err.to_string()))?;
                    return Ok(response.into());
                }
                result = write_workspace_command_stdin(&request_handle, process_id.as_str(), stdin) => {
                    result
                }
            };

            if let Err(err) = write_result
                && !exec_task.is_finished()
            {
                match tokio::time::timeout(STDIN_WRITE_FAILURE_RESULT_GRACE, &mut exec_task).await {
                    Ok(response) => {
                        let response = response
                            .map_err(|err| {
                                WorkspaceCommandError::new(format!("command task failed: {err}"))
                            })?
                            .map_err(|err| WorkspaceCommandError::new(err.to_string()))?;
                        return Ok(response.into());
                    }
                    Err(_) => {
                        let _ =
                            terminate_workspace_command(&request_handle, process_id.as_str()).await;
                        let _ = exec_task.await;
                        return Err(err);
                    }
                }
            }

            let response = exec_task
                .await
                .map_err(|err| WorkspaceCommandError::new(format!("command task failed: {err}")))?
                .map_err(|err| WorkspaceCommandError::new(err.to_string()))?;
            Ok(response.into())
        })
    }
}

impl From<CommandExecResponse> for WorkspaceCommandOutput {
    fn from(response: CommandExecResponse) -> Self {
        Self {
            exit_code: response.exit_code,
            stdout: response.stdout,
            stderr: response.stderr,
        }
    }
}

async fn write_workspace_command_stdin(
    request_handle: &AppServerRequestHandle,
    process_id: &str,
    stdin: Vec<u8>,
) -> Result<(), WorkspaceCommandError> {
    let delta_base64 = STANDARD.encode(stdin);
    let mut last_error = None;
    for _ in 0..STDIN_WRITE_MAX_ATTEMPTS {
        let result: Result<CommandExecWriteResponse, _> = request_handle
            .request_typed(ClientRequest::CommandExecWrite {
                request_id: RequestId::String(format!(
                    "workspace-command-stdin-{}",
                    Uuid::new_v4()
                )),
                params: CommandExecWriteParams {
                    process_id: process_id.to_string(),
                    delta_base64: Some(delta_base64.clone()),
                    close_stdin: true,
                },
            })
            .await;
        match result {
            Ok(_) => return Ok(()),
            Err(err) => {
                let message = err.to_string();
                if !command_exec_write_error_retryable(&err) {
                    return Err(WorkspaceCommandError::new(format!(
                        "failed to write command stdin: {message}"
                    )));
                }
                last_error = Some(message);
                tokio::time::sleep(STDIN_WRITE_RETRY_DELAY).await;
            }
        }
    }

    Err(WorkspaceCommandError::new(format!(
        "failed to write command stdin: {}",
        last_error.unwrap_or_else(|| "unknown error".to_string())
    )))
}

fn command_exec_write_error_retryable(err: &TypedRequestError) -> bool {
    match err {
        TypedRequestError::Server { method, source } => {
            method == "command/exec/write"
                && source
                    .data
                    .as_ref()
                    .and_then(|data| data.get("reason"))
                    .and_then(serde_json::Value::as_str)
                    == Some("command_exec_not_active")
        }
        TypedRequestError::Transport { .. } | TypedRequestError::Deserialize { .. } => false,
    }
}

async fn terminate_workspace_command(
    request_handle: &AppServerRequestHandle,
    process_id: &str,
) -> Result<(), WorkspaceCommandError> {
    let _: CommandExecTerminateResponse = request_handle
        .request_typed(ClientRequest::CommandExecTerminate {
            request_id: RequestId::String(format!("workspace-command-stop-{}", Uuid::new_v4())),
            params: CommandExecTerminateParams {
                process_id: process_id.to_string(),
            },
        })
        .await
        .map_err(|err| WorkspaceCommandError::new(err.to_string()))?;
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "workspace_command_tests.rs"]
mod tests;
