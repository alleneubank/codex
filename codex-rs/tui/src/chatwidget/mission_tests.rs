use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use pretty_assertions::assert_eq;

use super::MISSION_OUTPUT_BYTES;
use super::mission_command;
use super::run_mission_command;
use crate::app_event::AppEvent;
use crate::slash_command::SlashCommand;
use crate::workspace_command::WorkspaceCommand;
use crate::workspace_command::WorkspaceCommandError;
use crate::workspace_command::WorkspaceCommandExecutor;
use crate::workspace_command::WorkspaceCommandOutput;
use crate::workspace_command::WorkspaceCommandOutputCap;

#[test]
fn mission_command_maps_bounded_projection_views() {
    let root = Path::new("/workspace");

    for (args, expected) in [
        ("", vec!["missionctl", "context", "--root", "/workspace"]),
        (
            "context",
            vec!["missionctl", "context", "--root", "/workspace"],
        ),
        ("check", vec!["missionctl", "check", "--root", "/workspace"]),
        (
            "mission",
            vec!["missionctl", "mission", "--root", "/workspace"],
        ),
        (
            "inspect",
            vec!["missionctl", "inspect", "--root", "/workspace"],
        ),
    ] {
        let command = mission_command(args, root).expect("valid mission command");
        assert_eq!(command.argv, expected);
        assert_eq!(command.cwd.as_deref(), Some(root));
        assert_eq!(command.timeout, Duration::from_secs(/*secs*/ 10));
        assert_eq!(
            command.output_cap,
            WorkspaceCommandOutputCap::Bytes(MISSION_OUTPUT_BYTES)
        );
    }
}

#[tokio::test]
async fn mission_command_rejects_empty_success_output() {
    let runner = StubRunner::new(WorkspaceCommandOutput {
        exit_code: 0,
        stdout: " \n\t".to_string(),
        stderr: String::new(),
    });

    let result = run_mission_command(&runner, Path::new("/workspace"), "context").await;

    assert_eq!(result, Err("missionctl returned no output".to_string()));
}

#[test]
fn mission_command_rejects_unknown_or_extra_arguments() {
    let root = Path::new("/workspace");

    let expected = "Usage: /mission [context|check|mission|inspect]";
    assert_eq!(mission_command("score", root).unwrap_err(), expected);
    assert_eq!(mission_command("portfolio", root).unwrap_err(), expected);
    assert_eq!(
        mission_command("prompt resume", root).unwrap_err(),
        expected
    );
    assert_eq!(
        mission_command("context extra", root).unwrap_err(),
        expected
    );
}

#[tokio::test]
async fn mission_command_surfaces_verifier_failures() {
    let runner = StubRunner::new(WorkspaceCommandOutput {
        exit_code: 2,
        stdout: String::new(),
        stderr: "LOOP.md: error target.spec-unknown targets.spec[0]: REQ-QUALITY-404 is not in SPEC.md\n".to_string(),
    });

    let result = run_mission_command(&runner, Path::new("/workspace"), "context").await;

    assert_eq!(
        result,
        Err(
            "missionctl failed (exit 2): LOOP.md: error target.spec-unknown targets.spec[0]: REQ-QUALITY-404 is not in SPEC.md".to_string()
        )
    );
}

#[tokio::test]
async fn mission_slash_command_dispatches_workspace_projection() {
    let (mut chat, _tx, mut rx, _op_rx) =
        crate::chatwidget::tests::make_chatwidget_manual_with_sender().await;
    let runner = Arc::new(StubRunner::new(WorkspaceCommandOutput {
        exit_code: 0,
        stdout: "LOOP widget-pagination [active] phase=TDD iteration=3/8\n".to_string(),
        stderr: String::new(),
    }));
    chat.workspace_command_runner = Some(runner.clone());
    chat.current_cwd = Some("/workspace".into());

    chat.dispatch_command_with_args(SlashCommand::Mission, "context".to_string(), Vec::new());

    let event = tokio::time::timeout(std::time::Duration::from_secs(/*secs*/ 1), rx.recv())
        .await
        .expect("mission result timeout")
        .expect("mission result channel");
    assert_matches::assert_matches!(
        event,
        AppEvent::MissionResult { cwd, result }
            if cwd == Path::new("/workspace")
                && result == Ok("LOOP widget-pagination [active] phase=TDD iteration=3/8".to_string())
    );
    let commands = runner.commands.lock().expect("commands mutex");
    let command = commands.first().expect("captured mission command");
    assert_eq!(
        command.argv,
        vec!["missionctl", "context", "--root", "/workspace"]
    );
}

struct StubRunner {
    output: WorkspaceCommandOutput,
    commands: Mutex<Vec<WorkspaceCommand>>,
}

impl StubRunner {
    fn new(output: WorkspaceCommandOutput) -> Self {
        Self {
            output,
            commands: Mutex::new(Vec::new()),
        }
    }
}

impl WorkspaceCommandExecutor for StubRunner {
    fn run(
        &self,
        command: WorkspaceCommand,
    ) -> Pin<
        Box<dyn Future<Output = Result<WorkspaceCommandOutput, WorkspaceCommandError>> + Send + '_>,
    > {
        self.commands.lock().expect("commands mutex").push(command);
        Box::pin(async { Ok(self.output.clone()) })
    }
}
