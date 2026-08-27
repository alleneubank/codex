use std::path::PathBuf;

use codex_protocol::ThreadId;
use codex_protocol::protocol::HookCompletedEvent;
use codex_protocol::protocol::HookEventName;
use codex_protocol::protocol::HookOutputEntry;
use codex_protocol::protocol::HookOutputEntryKind;
use codex_protocol::protocol::HookRunStatus;
use codex_protocol::protocol::HookRunSummary;
use codex_utils_absolute_path::AbsolutePathBuf;

use super::common;
use crate::engine::ClaudeHooksEngine;
use crate::engine::ConfiguredHandler;
use crate::engine::HandlerRunResult;
use crate::engine::dispatcher;
use crate::schema::NotificationCommandInput;
use crate::schema::NullableString;

const INPUT_NEEDED_MESSAGE: &str = "Codex needs your input";
const INPUT_COMPLETE_MESSAGE: &str = "Codex input request completed";

/// A bounded user-attention lifecycle notification exposed to observer hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationType {
    UserInputRequest,
    UserInputComplete,
    ElicitationDialog,
    ElicitationUrlDialog,
    ElicitationComplete,
    PlanImplementationRequest,
    PlanImplementationComplete,
}

impl NotificationType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserInputRequest => "user_input_request",
            Self::UserInputComplete => "user_input_complete",
            Self::ElicitationDialog => "elicitation_dialog",
            Self::ElicitationUrlDialog => "elicitation_url_dialog",
            Self::ElicitationComplete => "elicitation_complete",
            Self::PlanImplementationRequest => "plan_implementation_request",
            Self::PlanImplementationComplete => "plan_implementation_complete",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::UserInputRequest
            | Self::ElicitationDialog
            | Self::ElicitationUrlDialog
            | Self::PlanImplementationRequest => INPUT_NEEDED_MESSAGE,
            Self::UserInputComplete
            | Self::ElicitationComplete
            | Self::PlanImplementationComplete => INPUT_COMPLETE_MESSAGE,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NotificationRequest {
    pub session_id: ThreadId,
    /// Internal correlation for public hook lifecycle events; never serialized to hook stdin.
    pub turn_id: String,
    pub cwd: AbsolutePathBuf,
    pub transcript_path: Option<PathBuf>,
    pub model: String,
    pub permission_mode: String,
    pub notification_type: NotificationType,
}

#[derive(Debug, Default)]
pub struct NotificationOutcome {
    pub hook_events: Vec<HookCompletedEvent>,
}

pub(crate) fn preview(
    handlers: &[ConfiguredHandler],
    request: &NotificationRequest,
) -> Vec<HookRunSummary> {
    dispatcher::select_handlers(
        handlers,
        HookEventName::Notification,
        Some(request.notification_type.as_str()),
    )
    .into_iter()
    .map(|handler| dispatcher::running_summary(&handler))
    .collect()
}

pub(crate) async fn run(
    engine: &ClaudeHooksEngine,
    request: NotificationRequest,
) -> NotificationOutcome {
    let matched = dispatcher::select_handlers(
        &engine.handlers,
        HookEventName::Notification,
        Some(request.notification_type.as_str()),
    );
    if matched.is_empty() {
        return NotificationOutcome::default();
    }

    let input_json = match serde_json::to_string(&NotificationCommandInput {
        session_id: request.session_id.to_string(),
        transcript_path: NullableString::from_path(request.transcript_path.clone()),
        cwd: request.cwd.display().to_string(),
        hook_event_name: "Notification".to_string(),
        model: request.model,
        permission_mode: request.permission_mode,
        notification_type: request.notification_type.as_str().to_string(),
        message: request.notification_type.message().to_string(),
        title: None,
    }) {
        Ok(input_json) => input_json,
        Err(error) => {
            return NotificationOutcome {
                hook_events: common::serialization_failure_hook_events(
                    matched,
                    Some(request.turn_id),
                    format!("failed to serialize notification hook input: {error}"),
                ),
            };
        }
    };

    let results = dispatcher::execute_handlers(
        engine,
        matched,
        input_json,
        request.cwd.as_path(),
        Some(request.turn_id),
        parse_completed,
    )
    .await;
    NotificationOutcome {
        hook_events: results.into_iter().map(|result| result.completed).collect(),
    }
}

fn parse_completed(
    handler: &ConfiguredHandler,
    run_result: HandlerRunResult,
    turn_id: Option<String>,
) -> dispatcher::ParsedHandler<()> {
    let (status, entries) = match (run_result.error.as_deref(), run_result.exit_code) {
        (Some(error), _) => (
            HookRunStatus::Failed,
            vec![HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text: error.to_string(),
            }],
        ),
        (None, Some(0)) => (HookRunStatus::Completed, Vec::new()),
        (None, Some(code)) => (
            HookRunStatus::Failed,
            vec![HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text: common::trimmed_non_empty(&run_result.stderr)
                    .unwrap_or_else(|| format!("hook exited with code {code}")),
            }],
        ),
        (None, None) => (
            HookRunStatus::Failed,
            vec![HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text: "hook process terminated without an exit code".to_string(),
            }],
        ),
    };

    dispatcher::ParsedHandler {
        completed: HookCompletedEvent {
            turn_id,
            run: dispatcher::completed_summary(handler, &run_result, status, entries),
        },
        data: (),
        completion_order: 0,
    }
}

#[cfg(test)]
#[path = "notification_tests.rs"]
mod tests;
