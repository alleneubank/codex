#[cfg(target_os = "linux")]
use std::collections::HashMap;

#[cfg(target_os = "linux")]
use codex_protocol::protocol::HookEventName;
#[cfg(target_os = "linux")]
use codex_protocol::protocol::HookSource;
#[cfg(target_os = "linux")]
use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;

#[cfg(target_os = "linux")]
use super::super::CommandShell;
#[cfg(target_os = "linux")]
use super::super::ConfiguredHandler;
#[cfg(target_os = "linux")]
use super::run_command;

#[cfg(target_os = "linux")]
fn handler(command: String) -> ConfiguredHandler {
    ConfiguredHandler {
        event_name: HookEventName::Stop,
        matcher: None,
        command,
        timeout_sec: 5,
        status_message: None,
        source_path: AbsolutePathBuf::current_dir().expect("current dir"),
        source: HookSource::Project,
        display_order: 0,
        env: HashMap::new(),
    }
}

// Linux /bin/sh reports an unopened script with exit code 2. Darwin reports
// 127 instead, which already follows the ordinary nonzero-exit failure path.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn missing_shell_script_is_reported_as_hook_execution_error() {
    let missing_script = tempfile::tempdir()
        .expect("temp dir")
        .path()
        .join("missing-hook.sh");
    let result = run_command(
        &CommandShell {
            program: String::new(),
            args: Vec::new(),
        },
        &handler(format!("sh {}", missing_script.display())),
        0,
        "{}",
        std::path::Path::new("/"),
    )
    .await;

    assert_eq!(result.exit_code, Some(2));
    assert!(result.stderr.contains("cannot open"));
    assert!(
        result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("hook command failed to open script")),
        "error: {:?}",
        result.error
    );
}

#[test]
fn exit_two_for_missing_shell_script_is_reclassified() {
    let stderr = "sh: 0: cannot open /tmp/missing-hook.sh: No such file";

    assert_eq!(
        super::shell_script_open_error(Some(2), stderr),
        Some(format!("hook command failed to open script: {stderr}")),
    );
}

#[test]
fn exit_two_with_regular_feedback_is_not_reclassified() {
    assert_eq!(
        super::shell_script_open_error(Some(2), "retry with tests"),
        None
    );
}
