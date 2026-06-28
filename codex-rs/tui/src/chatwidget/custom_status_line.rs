//! Command-backed custom status line snapshots.
//!
//! The custom status line is a semantic snapshot renderer. It runs when the TUI
//! has new session/status state to show and never refreshes itself on a fixed
//! timer. Failed, timed-out, or empty command output hides the custom row.

use super::*;
use crate::workspace_command::WorkspaceCommand;
use crate::workspace_command::WorkspaceCommandPlatform;
use crate::workspace_command::WorkspaceCommandRunner;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use codex_ansi_escape::ansi_escape_line;
use codex_config::types::CustomStatusLineConfig;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_protocol::models::ActivePermissionProfile;
use codex_protocol::models::BUILT_IN_PERMISSION_PROFILE_DANGER_FULL_ACCESS;
use codex_protocol::models::BUILT_IN_PERMISSION_PROFILE_READ_ONLY;
use codex_protocol::models::BUILT_IN_PERMISSION_PROFILE_WORKSPACE;
use codex_protocol::models::ManagedFileSystemPermissions;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::NetworkSandboxPolicy;
use serde_json::Value;
use serde_json::json;

const CUSTOM_STATUS_LINE_TIMEOUT: Duration = Duration::from_secs(5);
const CUSTOM_STATUS_LINE_MAX_STDOUT_BYTES: usize = 4096;
const CUSTOM_STATUS_LINE_MAX_PADDING: u16 = 2;
const CUSTOM_STATUS_LINE_GOAL_OBJECTIVE_MAX_CHARS: usize = 240;
const DEFAULT_TERMINAL_COLUMNS: u16 = 80;
const WINDOWS_CMD_EXE: &str = r"C:\Windows\System32\cmd.exe";
const WINDOWS_POWERSHELL_EXE: &str = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";

#[derive(Default)]
pub(super) struct CustomStatusLineState {
    next_request_id: u64,
    pending: Option<tokio::task::JoinHandle<()>>,
    pending_request_id: Option<u64>,
    queued: Option<CustomStatusLineRequest>,
    last_request_key: Option<CustomStatusLineRequestKey>,
    project_dir_cache: Option<CustomStatusLineProjectDirCache>,
}

impl Drop for CustomStatusLineState {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.abort();
        }
    }
}

struct CustomStatusLineRequest {
    request_id: u64,
    config: CustomStatusLineConfig,
    cwd: PathBuf,
    payload: Value,
    columns: u16,
    runner: WorkspaceCommandRunner,
    platform: WorkspaceCommandPlatform,
}

#[derive(Clone, PartialEq)]
struct CustomStatusLineRequestKey {
    config: CustomStatusLineConfig,
    cwd: PathBuf,
    columns: u16,
    platform: WorkspaceCommandPlatform,
    payload: Value,
}

#[derive(Clone)]
struct CustomStatusLineProjectDirCache {
    cwd: PathBuf,
    project_dir: PathBuf,
}

impl ChatWidget {
    pub(super) fn refresh_custom_status_line(&mut self) {
        let Some(config) = self.config.tui_custom_status_line.clone() else {
            self.invalidate_custom_status_line_requests();
            self.bottom_pane
                .set_custom_status_line(/*status_line*/ None, /*padding*/ 0);
            return;
        };

        if config.command.trim().is_empty() {
            self.invalidate_custom_status_line_requests();
            self.bottom_pane
                .set_custom_status_line(/*status_line*/ None, /*padding*/ 0);
            return;
        }

        let Some(runner) = self.workspace_command_runner.clone() else {
            self.invalidate_custom_status_line_requests();
            self.bottom_pane
                .set_custom_status_line(/*status_line*/ None, /*padding*/ 0);
            return;
        };

        let cwd = self.status_line_cwd().to_path_buf();
        let columns = self
            .last_rendered_width
            .get()
            .and_then(|width| u16::try_from(width).ok())
            .filter(|width| *width > 0)
            .unwrap_or(DEFAULT_TERMINAL_COLUMNS);
        let platform = runner.platform();
        let payload = self.custom_status_line_payload(&cwd);
        let request_key = CustomStatusLineRequestKey {
            config: config.clone(),
            cwd: cwd.clone(),
            columns,
            platform,
            payload: payload.clone(),
        };
        if self.custom_status_line_state.last_request_key.as_ref() == Some(&request_key) {
            return;
        }
        self.custom_status_line_state.last_request_key = Some(request_key);

        self.custom_status_line_state.next_request_id = self
            .custom_status_line_state
            .next_request_id
            .saturating_add(1);
        let request_id = self.custom_status_line_state.next_request_id;

        let request = CustomStatusLineRequest {
            request_id,
            config,
            cwd,
            payload,
            columns,
            runner,
            platform,
        };
        if self.custom_status_line_state.pending.is_some() {
            self.custom_status_line_state.queued = Some(request);
            return;
        }
        self.spawn_custom_status_line_request(request);
    }

    pub(super) fn force_refresh_custom_status_line(&mut self) {
        self.custom_status_line_state.last_request_key = None;
        self.refresh_custom_status_line();
    }

    pub(crate) fn apply_custom_status_line_rendered(
        &mut self,
        request_id: u64,
        result: Option<String>,
    ) {
        if self.custom_status_line_state.pending_request_id != Some(request_id) {
            tracing::debug!(request_id, "ignored stale custom status line result");
            return;
        }
        self.custom_status_line_state.pending = None;
        self.custom_status_line_state.pending_request_id = None;

        if let Some(request) = self.custom_status_line_state.queued.take() {
            tracing::debug!(
                request_id,
                "ignored superseded custom status line result before queued render"
            );
            self.spawn_custom_status_line_request(request);
            return;
        }

        if request_id == self.custom_status_line_state.next_request_id {
            let padding = self
                .config
                .tui_custom_status_line
                .as_ref()
                .map(|config| config.padding.min(CUSTOM_STATUS_LINE_MAX_PADDING))
                .unwrap_or_default();
            let line =
                result.and_then(|output| first_renderable_line(&output).map(ansi_escape_line));
            if line.is_none() {
                self.custom_status_line_state.last_request_key = None;
            }
            self.bottom_pane.set_custom_status_line(line, padding);
        } else {
            tracing::debug!(request_id, "ignored superseded custom status line result");
        }
    }

    fn invalidate_custom_status_line_requests(&mut self) {
        self.custom_status_line_state.next_request_id = self
            .custom_status_line_state
            .next_request_id
            .saturating_add(1);
        self.custom_status_line_state.queued = None;
        self.custom_status_line_state.last_request_key = None;
    }

    pub(super) fn invalidate_custom_status_line_project_dir_cache(&mut self) {
        self.custom_status_line_state.project_dir_cache = None;
    }

    fn spawn_custom_status_line_request(&mut self, request: CustomStatusLineRequest) {
        let CustomStatusLineRequest {
            request_id,
            config,
            cwd,
            payload,
            columns,
            runner,
            platform,
        } = request;
        let tx = self.app_event_tx.clone();
        self.custom_status_line_state.pending_request_id = Some(request_id);
        self.custom_status_line_state.pending = Some(tokio::spawn(async move {
            let result =
                render_custom_status_line_command(config, cwd, payload, columns, runner, platform)
                    .await;
            match &result {
                Some(output) => {
                    tracing::debug!(
                        request_id,
                        bytes = output.len(),
                        "custom status line rendered"
                    );
                }
                None => {
                    tracing::debug!(
                        request_id,
                        "custom status line produced no renderable output"
                    );
                }
            }
            tx.send(AppEvent::CustomStatusLineRendered { request_id, result });
        }));
    }

    fn custom_status_line_payload(&mut self, cwd: &Path) -> Value {
        let project_dir = self.custom_status_line_project_dir(cwd);
        let total_usage = self.status_line_total_usage();
        let last_usage = self
            .token_info
            .as_ref()
            .map(|info| info.last_token_usage.clone())
            .unwrap_or_default();

        json!({
            "hook_event_name": "Status",
            "session_id": self.thread_id.map(|thread_id| thread_id.to_string()),
            "version": CODEX_CLI_VERSION,
            "workspace": {
                "current_dir": cwd.display().to_string(),
                "project_dir": project_dir.display().to_string(),
            },
            "model": {
                "id": self.current_model(),
                "display_name": self.model_display_name(),
            },
            "context_window": {
                "current_usage": token_usage_payload(&last_usage),
                "context_window_size": self.status_line_context_window_size(),
                "used_percentage": self.status_line_context_used_percent(),
                "total_input_tokens": total_usage.input_tokens,
                "total_output_tokens": total_usage.output_tokens,
                "total_tokens": total_usage.total_tokens,
            },
            "cost": {
                "total_cost_usd": null,
                "total_duration_ms": self.custom_status_line_total_duration_ms(),
            },
            "permissions": self.custom_status_line_permissions_payload(),
            "goal": self.custom_status_line_goal_payload(),
        })
    }

    fn custom_status_line_permissions_payload(&self) -> Value {
        let approval_policy = AskForApproval::from(self.config.permissions.approval_policy.value());
        let permission_profile = self.config.permissions.effective_permission_profile();
        let active_permission_profile = self.config.permissions.active_permission_profile();
        let approvals_reviewer = self.config.approvals_reviewer;
        let yolo = has_yolo_permissions(approval_policy, &permission_profile);
        let mode = custom_status_line_permission_mode(
            approval_policy,
            approvals_reviewer,
            &permission_profile,
            active_permission_profile.as_ref(),
            yolo,
        );

        json!({
            "mode": mode.id,
            "label": mode.label,
            "approval_policy": approval_policy.to_core().to_string(),
            "approvals_reviewer": approvals_reviewer.to_string(),
            "active_profile_id": active_permission_profile
                .as_ref()
                .map(|profile| profile.id.as_str()),
            "active_profile_extends": active_permission_profile
                .as_ref()
                .and_then(|profile| profile.extends.as_deref()),
            "file_system": permission_profile_file_system_label(&permission_profile),
            "network": permission_profile.network_sandbox_policy().to_string(),
            "enforcement": permission_profile.enforcement(),
            "yolo": yolo,
        })
    }

    fn custom_status_line_project_dir(&mut self, cwd: &Path) -> PathBuf {
        if let Some(cache) = &self.custom_status_line_state.project_dir_cache
            && cache.cwd == cwd
        {
            return cache.project_dir.clone();
        }

        let project_dir = self
            .status_line_project_root_for_cwd(cwd)
            .unwrap_or_else(|| self.config.cwd.as_path().to_path_buf());
        self.custom_status_line_state.project_dir_cache = Some(CustomStatusLineProjectDirCache {
            cwd: cwd.to_path_buf(),
            project_dir: project_dir.clone(),
        });
        project_dir
    }

    fn custom_status_line_goal_payload(&self) -> Value {
        let Some(goal_status) = self.current_goal_status.as_ref() else {
            return Value::Null;
        };
        let goal = goal_status.snapshot(
            Instant::now(),
            self.turn_lifecycle.goal_status_active_turn_started_at,
        );

        json!({
            "objective": truncate_goal_objective(&goal.objective),
            "status": goal.status,
            "token_budget": goal.token_budget,
            "tokens_used": goal.tokens_used,
            "time_used_seconds": goal.time_used_seconds,
        })
    }

    fn custom_status_line_total_duration_ms(&self) -> Option<u64> {
        let duration = self
            .turn_runtime_metrics
            .api_calls
            .duration_ms
            .saturating_add(self.turn_runtime_metrics.tool_calls.duration_ms)
            .saturating_add(self.turn_runtime_metrics.websocket_calls.duration_ms)
            .saturating_add(self.turn_runtime_metrics.responses_api_inference_time_ms)
            .saturating_add(self.turn_runtime_metrics.responses_api_overhead_ms);
        (duration > 0).then_some(duration)
    }
}

fn token_usage_payload(usage: &TokenUsage) -> Value {
    json!({
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "cache_read_input_tokens": usage.cached_input_tokens,
        "cache_creation_input_tokens": 0,
        "total_tokens": usage.total_tokens,
    })
}

fn permission_profile_file_system_label(permission_profile: &PermissionProfile) -> &'static str {
    match permission_profile {
        PermissionProfile::Managed {
            file_system: ManagedFileSystemPermissions::Restricted { .. },
            ..
        } => "restricted",
        PermissionProfile::Managed {
            file_system: ManagedFileSystemPermissions::Unrestricted,
            ..
        } => "unrestricted",
        PermissionProfile::Disabled => "unrestricted",
        PermissionProfile::External { .. } => "external",
    }
}

fn has_yolo_permissions(
    approval_policy: AskForApproval,
    permission_profile: &PermissionProfile,
) -> bool {
    approval_policy == AskForApproval::Never
        && matches!(
            permission_profile,
            PermissionProfile::Disabled
                | PermissionProfile::Managed {
                    file_system: ManagedFileSystemPermissions::Unrestricted,
                    network: NetworkSandboxPolicy::Enabled,
                }
        )
}

struct CustomStatusLinePermissionMode {
    id: &'static str,
    label: String,
}

fn custom_status_line_permission_mode(
    approval_policy: AskForApproval,
    approvals_reviewer: ApprovalsReviewer,
    permission_profile: &PermissionProfile,
    active_permission_profile: Option<&ActivePermissionProfile>,
    yolo: bool,
) -> CustomStatusLinePermissionMode {
    if yolo {
        return CustomStatusLinePermissionMode {
            id: "yolo",
            label: "YOLO".to_string(),
        };
    }

    if matches_builtin_profile(
        active_permission_profile,
        permission_profile,
        BUILT_IN_PERMISSION_PROFILE_READ_ONLY,
        PermissionProfile::read_only,
    ) {
        return CustomStatusLinePermissionMode {
            id: "read-only",
            label: "Read Only".to_string(),
        };
    }

    if matches_builtin_profile(
        active_permission_profile,
        permission_profile,
        BUILT_IN_PERMISSION_PROFILE_WORKSPACE,
        PermissionProfile::workspace_write,
    ) && approval_policy == AskForApproval::OnRequest
    {
        return match approvals_reviewer {
            ApprovalsReviewer::AutoReview => CustomStatusLinePermissionMode {
                id: "auto",
                label: "Auto".to_string(),
            },
            ApprovalsReviewer::User => CustomStatusLinePermissionMode {
                id: "ask",
                label: "Ask".to_string(),
            },
        };
    }

    if active_permission_profile
        .as_ref()
        .is_some_and(|profile| profile.id == BUILT_IN_PERMISSION_PROFILE_DANGER_FULL_ACCESS)
        || permission_profile == &PermissionProfile::Disabled
    {
        return CustomStatusLinePermissionMode {
            id: "full-access",
            label: "Full Access".to_string(),
        };
    }

    CustomStatusLinePermissionMode {
        id: "custom",
        label: active_permission_profile
            .filter(|profile| !profile.id.starts_with(':'))
            .map(|profile| profile.id.clone())
            .unwrap_or_else(|| "Custom".to_string()),
    }
}

fn matches_builtin_profile(
    active_permission_profile: Option<&ActivePermissionProfile>,
    permission_profile: &PermissionProfile,
    builtin_id: &str,
    builtin_profile: fn() -> PermissionProfile,
) -> bool {
    active_permission_profile.is_some_and(|profile| profile.id == builtin_id)
        || permission_profile == &builtin_profile()
}

fn truncate_goal_objective(objective: &str) -> String {
    if objective.chars().count() <= CUSTOM_STATUS_LINE_GOAL_OBJECTIVE_MAX_CHARS {
        return objective.to_string();
    }

    objective
        .chars()
        .take(CUSTOM_STATUS_LINE_GOAL_OBJECTIVE_MAX_CHARS.saturating_sub(3))
        .chain("...".chars())
        .collect()
}

async fn render_custom_status_line_command(
    config: CustomStatusLineConfig,
    cwd: PathBuf,
    payload: Value,
    columns: u16,
    runner: WorkspaceCommandRunner,
    platform: WorkspaceCommandPlatform,
) -> Option<String> {
    let payload = match serde_json::to_vec(&payload) {
        Ok(payload) => payload,
        Err(err) => {
            tracing::debug!(error = %err, "failed to serialize custom status line payload");
            return None;
        }
    };
    if !custom_status_line_command_allowed(config.command.as_str(), platform) {
        tracing::debug!("custom status line command is not allowed on this platform");
        return None;
    }

    let output = runner
        .run(custom_status_line_workspace_command(
            config.command,
            config.env,
            cwd,
            payload,
            columns,
            platform,
        ))
        .await;

    match output {
        Ok(output) if output.success() => {
            Some(output.stdout).filter(|output| first_renderable_line(output).is_some())
        }
        Ok(output) => {
            tracing::debug!(
                status = output.exit_code,
                "custom status line command exited non-zero"
            );
            None
        }
        Err(err) => {
            tracing::debug!(error = %err, "custom status line command failed");
            None
        }
    }
}

fn custom_status_line_command_allowed(command: &str, platform: WorkspaceCommandPlatform) -> bool {
    match platform {
        WorkspaceCommandPlatform::Unknown => return false,
        WorkspaceCommandPlatform::Unix => return true,
        WorkspaceCommandPlatform::Windows => {}
    }

    let command = command.trim_start();
    let program = if let Some(rest) = command.strip_prefix('"') {
        let Some(end) = rest.find('"') else {
            return false;
        };
        &rest[..end]
    } else {
        let Some(program) = command.split_whitespace().next() else {
            return false;
        };
        program
    };
    let bytes = program.as_bytes();
    if program.starts_with(r"\\") {
        return true;
    }
    let Some(drive_letter) = bytes.first() else {
        return false;
    };
    matches!(bytes.get(1), Some(b':'))
        && drive_letter.is_ascii_alphabetic()
        && matches!(bytes.get(2), Some(b'\\' | b'/'))
}

fn custom_status_line_workspace_command(
    command: String,
    env: BTreeMap<String, String>,
    cwd: PathBuf,
    payload: Vec<u8>,
    columns: u16,
    platform: WorkspaceCommandPlatform,
) -> WorkspaceCommand {
    let mut workspace_command =
        WorkspaceCommand::new(platform_shell_argv(command.clone(), platform))
            .cwd(cwd)
            .env("COLUMNS", columns.to_string())
            .env("LINES", "1")
            .timeout(CUSTOM_STATUS_LINE_TIMEOUT);
    if platform.is_windows() {
        // Windows restricted-token command/exec rejects streaming stdin and custom output caps.
        // Use a shell wrapper that decodes the bounded payload and pipes it into the configured
        // command while emitting only the first bounded status line through app-server's default
        // buffered cap.
        workspace_command = workspace_command.use_default_output_cap();
    } else {
        workspace_command = workspace_command
            .output_bytes_cap(CUSTOM_STATUS_LINE_MAX_STDOUT_BYTES)
            .stdin(payload.clone());
    }
    for (key, value) in env {
        workspace_command = workspace_command.env(key, value);
    }
    if platform.is_windows() {
        workspace_command = workspace_command
            .env("CODEX_STATUS_LINE_STDIN_BASE64", STANDARD.encode(payload))
            .env("CODEX_STATUS_LINE_COMMAND", command);
    }
    workspace_command
}

fn platform_shell_argv(command: String, platform: WorkspaceCommandPlatform) -> Vec<String> {
    if platform.is_windows() {
        let script = format!(
            concat!(
                "$payload = [Text.Encoding]::UTF8.GetString(",
                "[Convert]::FromBase64String($env:CODEX_STATUS_LINE_STDIN_BASE64)); ",
                "$firstLine = $null; ",
                "$sawLine = $false; ",
                "$commandExit = 0; ",
                "$payload | & '{windows_cmd_exe}' /C $env:CODEX_STATUS_LINE_COMMAND ",
                "| ForEach-Object {{ ",
                "if (-not $sawLine) {{ $firstLine = [string]$_; $sawLine = $true }} ",
                "}}; ",
                "$commandExit = $LASTEXITCODE; ",
                "if ($sawLine) {{ ",
                "$text = $firstLine; ",
                "if ($text.Length -gt {max_stdout_bytes}) {{ ",
                "$text.Substring(0, {max_stdout_bytes}) ",
                "}} else {{ $text }} ",
                "}}; ",
                "exit $commandExit",
            ),
            windows_cmd_exe = WINDOWS_CMD_EXE,
            max_stdout_bytes = CUSTOM_STATUS_LINE_MAX_STDOUT_BYTES
        );
        vec![
            WINDOWS_POWERSHELL_EXE.to_string(),
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-Command".to_string(),
            script,
        ]
    } else {
        vec!["sh".to_string(), "-c".to_string(), command]
    }
}

fn first_renderable_line(output: &str) -> Option<&str> {
    output
        .lines()
        .map(str::trim_end)
        .find(|line| !ansi_escape_line(line).to_string().trim().is_empty())
}

#[cfg(test)]
#[path = "custom_status_line_command_tests.rs"]
mod command_tests;

#[cfg(test)]
#[path = "custom_status_line_tests.rs"]
mod tests;
