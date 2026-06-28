use super::*;
use crate::workspace_command::WorkspaceCommand;
use crate::workspace_command::WorkspaceCommandError;
use crate::workspace_command::WorkspaceCommandExecutor;
use crate::workspace_command::WorkspaceCommandOutput;
use crate::workspace_command::WorkspaceCommandOutputCap;
use crate::workspace_command::WorkspaceCommandPlatform;
use codex_config::types::CustomStatusLineType;
use pretty_assertions::assert_eq;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

#[tokio::test]
async fn command_receives_stdin_and_env() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let runner = FakeStatusLineRunner::new(vec![WorkspaceCommandOutput {
        exit_code: 0,
        stdout: "ready\n".to_string(),
        stderr: String::new(),
    }]);
    let command_text = allowed_statusline_command(" --compact");
    let config = CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: command_text.clone(),
        env: BTreeMap::from([("STATUSLINE_LABEL".to_string(), "ready".to_string())]),
        padding: 0,
    };

    let output = render_custom_status_line_command(
        config,
        tempdir.path().to_path_buf(),
        json!({"workspace": {"current_dir": "/tmp"}}),
        /*columns*/ 120,
        runner.clone(),
        WorkspaceCommandPlatform::Unix,
    )
    .await;

    assert_eq!(
        output.as_deref().and_then(first_renderable_line),
        Some("ready")
    );
    let commands = runner.commands();
    assert_eq!(commands.len(), 1);
    let command = &commands[0];
    assert_eq!(
        command.argv,
        platform_shell_argv(command_text, WorkspaceCommandPlatform::Unix)
    );
    assert_eq!(command.cwd, Some(tempdir.path().to_path_buf()));
    assert_eq!(CUSTOM_STATUS_LINE_TIMEOUT, Duration::from_secs(5));
    assert_eq!(command.timeout, CUSTOM_STATUS_LINE_TIMEOUT);
    assert_eq!(
        command.output_cap,
        WorkspaceCommandOutputCap::Bytes(CUSTOM_STATUS_LINE_MAX_STDOUT_BYTES)
    );
    assert_eq!(
        command.stdin.as_deref(),
        Some(br#"{"workspace":{"current_dir":"/tmp"}}"#.as_slice())
    );
    assert_eq!(command.permission_profile, None);
    assert_eq!(
        command.env.get("STATUSLINE_LABEL"),
        Some(&Some("ready".to_string()))
    );
    assert_eq!(command.env.get("COLUMNS"), Some(&Some("120".to_string())));
    assert_eq!(command.env.get("LINES"), Some(&Some("1".to_string())));
}

#[tokio::test]
async fn windows_command_uses_base64_env_wrapper_and_propagates_exit() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let runner = FakeStatusLineRunner::new(vec![WorkspaceCommandOutput {
        exit_code: 0,
        stdout: "ready\n".to_string(),
        stderr: String::new(),
    }]);
    let command_text = r"C:\codex-statusline.exe --compact".to_string();
    let config = CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: command_text.clone(),
        env: BTreeMap::new(),
        padding: 0,
    };

    let output = render_custom_status_line_command(
        config,
        tempdir.path().to_path_buf(),
        json!({"workspace": {"current_dir": "C:\\project"}}),
        /*columns*/ 120,
        runner.clone(),
        WorkspaceCommandPlatform::Windows,
    )
    .await;

    assert_eq!(output.as_deref(), Some("ready\n"));
    let commands = runner.commands();
    assert_eq!(commands.len(), 1);
    let command = &commands[0];
    assert_eq!(
        command.argv,
        platform_shell_argv(command_text.clone(), WorkspaceCommandPlatform::Windows)
    );
    assert_eq!(command.output_cap, WorkspaceCommandOutputCap::Default);
    assert_eq!(command.stdin, None);
    assert_eq!(
        command.env.get("CODEX_STATUS_LINE_COMMAND"),
        Some(&Some(command_text))
    );
    assert!(command.env.contains_key("CODEX_STATUS_LINE_STDIN_BASE64"));
    let script = command.argv.last().expect("script should be present");
    assert!(script.contains("exit $commandExit"));
    assert!(script.contains("ForEach-Object"));
    assert!(script.contains("$payload | & 'C:\\Windows\\System32\\cmd.exe' /C"));
    assert!(!script.contains("$payload | & {"));
    assert!(!script.contains("$output ="));
}

#[tokio::test]
async fn windows_command_failure_hides_output() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let runner = FakeStatusLineRunner::new(vec![WorkspaceCommandOutput {
        exit_code: 7,
        stdout: "should not render\n".to_string(),
        stderr: "failed\n".to_string(),
    }]);
    let config = CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: r"C:\codex-statusline.exe --compact".to_string(),
        env: BTreeMap::new(),
        padding: 0,
    };

    let output = render_custom_status_line_command(
        config,
        tempdir.path().to_path_buf(),
        json!({"workspace": {"current_dir": "C:\\project"}}),
        /*columns*/ 120,
        runner,
        WorkspaceCommandPlatform::Windows,
    )
    .await;

    assert_eq!(output, None);
}

#[tokio::test]
async fn unknown_platform_does_not_run_custom_status_line_command() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let runner = FakeStatusLineRunner::new(Vec::new());
    let config = CustomStatusLineConfig {
        kind: CustomStatusLineType::Command,
        command: allowed_statusline_command(""),
        env: BTreeMap::new(),
        padding: 0,
    };

    let output = render_custom_status_line_command(
        config,
        tempdir.path().to_path_buf(),
        json!({"workspace": {"current_dir": "/tmp"}}),
        /*columns*/ 120,
        runner.clone(),
        WorkspaceCommandPlatform::Unknown,
    )
    .await;

    assert_eq!(output, None);
    assert!(runner.commands().is_empty());
}

#[cfg(windows)]
fn allowed_statusline_command(args: &str) -> String {
    format!(r#"C:\codex-statusline.exe{args}"#)
}

#[cfg(not(windows))]
fn allowed_statusline_command(args: &str) -> String {
    format!("statusline-command{args}")
}

struct FakeStatusLineRunner {
    commands: Mutex<Vec<WorkspaceCommand>>,
    outputs: Mutex<VecDeque<WorkspaceCommandOutput>>,
}

impl FakeStatusLineRunner {
    fn new(outputs: Vec<WorkspaceCommandOutput>) -> Arc<Self> {
        Arc::new(Self {
            commands: Mutex::new(Vec::new()),
            outputs: Mutex::new(outputs.into()),
        })
    }

    fn commands(&self) -> Vec<WorkspaceCommand> {
        self.commands
            .lock()
            .expect("commands mutex should not be poisoned")
            .clone()
    }
}

impl WorkspaceCommandExecutor for FakeStatusLineRunner {
    fn run(
        &self,
        command: WorkspaceCommand,
    ) -> Pin<
        Box<dyn Future<Output = Result<WorkspaceCommandOutput, WorkspaceCommandError>> + Send + '_>,
    > {
        self.commands
            .lock()
            .expect("commands mutex should not be poisoned")
            .push(command);
        let output = self
            .outputs
            .lock()
            .expect("outputs mutex should not be poisoned")
            .pop_front()
            .expect("fake runner output should be available");
        Box::pin(async move { Ok(output) })
    }
}
