use std::path::Path;
use std::time::Duration;

use super::ChatWidget;
use crate::app_event::AppEvent;
use crate::workspace_command::WorkspaceCommand;
use crate::workspace_command::WorkspaceCommandExecutor;

const MISSION_OUTPUT_BYTES: usize = 32 * 1024;
const MISSION_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 10);
const MISSION_USAGE: &str =
    "Usage: /mission [current|rubric|evidence|portfolio|resume|decision-review|handoff|landing]";

impl ChatWidget {
    pub(super) fn dispatch_mission_command(&mut self, args: &str) {
        let cwd = self
            .current_cwd
            .clone()
            .unwrap_or_else(|| self.config.cwd.to_path_buf());
        let Some(runner) = self.workspace_command_runner.clone() else {
            self.add_error_message(
                "Failed to run missionctl: workspace command runner unavailable".to_string(),
            );
            return;
        };
        let args = args.to_string();
        let tx = self.app_event_tx.clone();
        let result_cwd = cwd.clone();
        tokio::spawn(async move {
            let result = run_mission_command(runner.as_ref(), &cwd, &args).await;
            tx.send(AppEvent::MissionResult {
                cwd: result_cwd,
                result,
            });
        });
    }
}

fn mission_command(args: &str, root: &Path) -> Result<WorkspaceCommand, String> {
    let mut argv = vec!["missionctl".to_string()];
    match args.split_whitespace().collect::<Vec<_>>().as_slice() {
        [] | ["current"] => argv.push("current".to_string()),
        ["rubric"] | ["evidence"] => argv.push("mission".to_string()),
        ["portfolio"] => argv.push("portfolio".to_string()),
        [action @ ("resume" | "decision-review" | "handoff" | "landing")] => {
            argv.push("prompt".to_string());
            argv.push((*action).to_string());
        }
        _ => return Err(MISSION_USAGE.to_string()),
    }
    argv.push("--root".to_string());
    argv.push(root.to_string_lossy().into_owned());
    Ok(WorkspaceCommand::new(argv)
        .cwd(root)
        .timeout(MISSION_TIMEOUT)
        .output_bytes_cap(MISSION_OUTPUT_BYTES))
}

async fn run_mission_command(
    runner: &dyn WorkspaceCommandExecutor,
    root: &Path,
    args: &str,
) -> Result<String, String> {
    let command = mission_command(args, root)?;
    let output = runner
        .run(command)
        .await
        .map_err(|error| format!("Failed to run missionctl: {error}"))?;
    if !output.success() {
        let detail = if output.stderr.trim().is_empty() {
            output.stdout.trim()
        } else {
            output.stderr.trim()
        };
        let detail = if detail.is_empty() {
            "no diagnostic output"
        } else {
            detail
        };
        return Err(format!(
            "missionctl failed (exit {}): {detail}",
            output.exit_code
        ));
    }
    let stdout = output.stdout.trim_end();
    if stdout.is_empty() {
        return Err("missionctl returned no output".to_string());
    }
    Ok(stdout.to_string())
}

#[cfg(test)]
#[path = "mission_tests.rs"]
mod tests;
