use super::*;
use crate::session::tests::make_session_and_context_with_rx;
use crate::state::ActiveTurn;
use codex_hooks::HooksConfig;
use codex_models_manager::model_info::model_info_from_slug;
use codex_protocol::approvals::NetworkPolicyAmendment;
use codex_protocol::protocol::EventMsg;
use pretty_assertions::assert_eq;
use std::time::Duration;
use tokio::time::timeout;

#[cfg(unix)]
#[derive(Clone, Copy)]
enum ExpectedPromptEvent {
    Command,
    Patch,
    Mcp,
}

#[cfg(unix)]
#[allow(
    clippy::expect_used,
    reason = "approval hook fixture construction should fail the focused test immediately"
)]
async fn observed_approval_session(
    include_sync_allow: bool,
) -> (
    Arc<Session>,
    Arc<TurnContext>,
    async_channel::Receiver<codex_protocol::protocol::Event>,
    PathBuf,
) {
    let (session, turn_context, events) = make_session_and_context_with_rx().await;
    let codex_home = turn_context.config.codex_home.as_path();
    std::fs::create_dir_all(codex_home).expect("recreate Codex home for hook fixture");
    let marker = codex_home.join("permission-request-observer-ran");
    let observer_script = codex_home.join("permission-request-observer.sh");
    std::fs::write(
        &observer_script,
        format!(
            "#!/bin/sh\ncat >/dev/null\nprintf 1 > {}\n",
            shlex::try_quote(marker.to_string_lossy().as_ref()).expect("quote observer marker")
        ),
    )
    .expect("write async observer fixture");

    let mut handlers = Vec::new();
    if include_sync_allow {
        let policy_script = codex_home.join("permission-request-policy.sh");
        std::fs::write(
            &policy_script,
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"hookSpecificOutput\":{\"hookEventName\":\"PermissionRequest\",\"decision\":{\"behavior\":\"allow\"}}}'\n",
        )
        .expect("write synchronous policy fixture");
        handlers.push(serde_json::json!({
            "type": "command",
            "command": format!(
                "/bin/sh {}",
                shlex::try_quote(policy_script.to_string_lossy().as_ref())
                    .expect("quote policy script")
            ),
        }));
    }
    handlers.push(serde_json::json!({
        "type": "command",
        "command": format!(
            "/bin/sh {}",
            shlex::try_quote(observer_script.to_string_lossy().as_ref())
                .expect("quote observer script")
        ),
        "async": true,
    }));
    std::fs::write(
        codex_home.join("hooks.json"),
        serde_json::json!({
            "hooks": {
                "PermissionRequest": [{ "hooks": handlers }],
            },
        })
        .to_string(),
    )
    .expect("write PermissionRequest hooks fixture");

    let hooks = session.hooks().reconfigured(HooksConfig {
        feature_enabled: true,
        bypass_hook_trust: true,
        config_layer_stack: Some(turn_context.config.config_layer_stack.clone()),
        ..HooksConfig::default()
    });
    session.services.hooks.store(Arc::new(hooks));
    *session.active_turn.lock().await = Some(ActiveTurn::default());
    (session, turn_context, events, marker)
}

#[cfg(unix)]
fn observer_approval_context(turn_context: Arc<TurnContext>, call_id: &str) -> ApprovalContext {
    ApprovalContext {
        review_context: GuardianReviewContext::from(turn_context),
        cancellation_token: None,
        call_id: call_id.to_string(),
        tool_name: ToolName::plain("observer_route_test"),
        strict_auto_review: false,
        approval_reason: None,
        retry_reason: None,
        network_approval_context: None,
    }
}

#[cfg(unix)]
#[allow(
    clippy::expect_used,
    reason = "missing prompt events and observer output are direct test failures"
)]
async fn assert_prompt_action_dispatches_observer(
    action: ApprovalAction,
    expected_event: ExpectedPromptEvent,
) {
    let (session, turn_context, events, marker) = observed_approval_session(false).await;
    let approval_context = observer_approval_context(Arc::clone(&turn_context), "observer-route");
    let approval_session = Arc::clone(&session);
    let approval_task = tokio::spawn(async move {
        approval_session
            .request_user_approval(&action, &approval_context, "observer-route")
            .await
    });

    timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.expect("approval event stream");
            let matches = match expected_event {
                ExpectedPromptEvent::Command => {
                    matches!(event.msg, EventMsg::ExecApprovalRequest(_))
                }
                ExpectedPromptEvent::Patch => {
                    matches!(event.msg, EventMsg::ApplyPatchApprovalRequest(_))
                }
                ExpectedPromptEvent::Mcp => matches!(
                    event.msg,
                    EventMsg::ElicitationRequest(_) | EventMsg::RequestUserInput(_)
                ),
            };
            if matches {
                break;
            }
        }
    })
    .await
    .expect("user approval prompt should be emitted");
    session.hooks().wait_for_async_hooks().await;
    assert_eq!(
        std::fs::read_to_string(&marker).expect("observer marker should exist"),
        "1"
    );

    approval_task.abort();
    let _ = approval_task.await;
}

#[cfg(unix)]
#[tokio::test]
#[allow(
    clippy::expect_used,
    reason = "invalid host paths make this Unix-only route test inapplicable"
)]
async fn every_distinct_user_approval_route_dispatches_async_observer() {
    let cwd = AbsolutePathBuf::try_from(std::env::current_dir().expect("current directory"))
        .expect("absolute current directory");
    let target = cwd.join("observer-route-test.txt");
    let target_uri = PathUri::from_abs_path(&target);
    let cwd_uri = PathUri::from_abs_path(&cwd);

    assert_prompt_action_dispatches_observer(
        ApprovalAction::Execve {
            id: "execve-route".to_string(),
            approval_id: "execve-route-approval".to_string(),
            environment_id: codex_exec_server::LOCAL_ENVIRONMENT_ID.to_string(),
            source: GuardianCommandSource::Shell,
            program: AbsolutePathBuf::from_absolute_path("/usr/bin/touch")
                .expect("absolute executable"),
            argv: vec!["touch".to_string(), target.to_string_lossy().into_owned()],
            command: vec!["touch".to_string(), target.to_string_lossy().into_owned()],
            cwd: cwd.clone(),
            additional_permissions: None,
        },
        ExpectedPromptEvent::Command,
    )
    .await;

    assert_prompt_action_dispatches_observer(
        ApprovalAction::ApplyPatch {
            id: "patch-route".to_string(),
            environment_id: codex_exec_server::LOCAL_ENVIRONMENT_ID.to_string(),
            cwd: cwd_uri,
            files: vec![target_uri],
            patch:
                "*** Begin Patch\n*** Add File: observer-route-test.txt\n+observed\n*** End Patch"
                    .to_string(),
            changes: Arc::new(HashMap::from([(
                target.clone().into_path_buf(),
                FileChange::Add {
                    content: "observed\n".to_string(),
                },
            )])),
            permissions_preapproved: false,
        },
        ExpectedPromptEvent::Patch,
    )
    .await;

    assert_prompt_action_dispatches_observer(
        ApprovalAction::McpToolCall {
            id: "mcp-route".to_string(),
            server: "observer-server".to_string(),
            tool_name: "observer-tool".to_string(),
            arguments: Some(serde_json::json!({ "value": 1 })),
            connector_id: None,
            connector_name: None,
            connector_description: None,
            connected_account_email: None,
            tool_title: Some("Observer Tool".to_string()),
            tool_description: None,
            annotations: None,
            hook_tool_name: HookToolName::new("mcp__observer__tool"),
            approval_policy: AskForApproval::OnRequest,
            reviewer: ApprovalsReviewer::User,
            approval_mode: AppToolApproval::Prompt,
            allow_session_remember: false,
            allow_persistent_approval: false,
        },
        ExpectedPromptEvent::Mcp,
    )
    .await;

    assert_prompt_action_dispatches_observer(
        ApprovalAction::NetworkAccess {
            id: "network-route".to_string(),
            turn_id: "turn-id".to_string(),
            environment_id: codex_exec_server::LOCAL_ENVIRONMENT_ID.to_string(),
            target: "https://observer.invalid".to_string(),
            host: "observer.invalid".to_string(),
            protocol: NetworkApprovalProtocol::Https,
            port: 443,
            trigger: None,
            hook_command: "curl https://observer.invalid".to_string(),
            hook_run_id: "network-route-hook".to_string(),
            command: vec!["curl".to_string(), "https://observer.invalid".to_string()],
            cwd,
        },
        ExpectedPromptEvent::Command,
    )
    .await;
}

#[cfg(unix)]
#[tokio::test]
#[allow(
    clippy::expect_used,
    reason = "a rejected synchronous allow is the assertion failure under test"
)]
async fn synchronous_policy_decision_suppresses_coexisting_async_observer() {
    let (session, turn_context, _events, marker) = observed_approval_session(true).await;
    let cwd = turn_context.config.cwd.clone();
    let action = ApprovalAction::ExecCommand {
        id: "policy-route".to_string(),
        environment_id: codex_exec_server::LOCAL_ENVIRONMENT_ID.to_string(),
        command: vec!["printf".to_string(), "approved".to_string()],
        hook_command: "printf approved".to_string(),
        cwd: PathUri::from_abs_path(&cwd),
        sandbox_permissions: SandboxPermissions::RequireEscalated,
        additional_permissions: None,
        justification: None,
        tty: false,
        proposed_execpolicy_amendment: None,
    };
    let decision = session
        .request_approval(
            action,
            observer_approval_context(turn_context, "policy-route"),
        )
        .await
        .expect("synchronous policy allow should approve the action");
    assert_eq!(decision, ReviewDecision::Approved);
    session.hooks().wait_for_async_hooks().await;
    assert!(
        !marker.exists(),
        "a synchronous PermissionRequest policy decision must suppress async observers"
    );
}

#[test]
fn approval_resolution_rejects_denied_network_policy_amendment() {
    let resolution = ApprovalResolution {
        decision: ReviewDecision::NetworkPolicyAmendment {
            network_policy_amendment: NetworkPolicyAmendment {
                host: "denied.example.com".to_string(),
                action: NetworkPolicyRuleAction::Deny,
            },
        },
        source: ApprovalResolutionSource::User,
    };

    assert!(matches!(
        resolution.into_tool_result(&model_info_from_slug("acting-model")),
        Err(ToolError::Rejected(rejection)) if rejection == "rejected by user"
    ));
}

#[test]
fn approval_resolution_rejects_mcp_policy_amendment() {
    let resolution = ApprovalResolution {
        decision: ReviewDecision::ApprovedMcpPolicyAmendment,
        source: ApprovalResolutionSource::User,
    };

    assert!(matches!(
        resolution.into_tool_result(&model_info_from_slug("acting-model")),
        Err(ToolError::Rejected(rejection)) if rejection == "Error while requesting approval"
    ));
}

#[test]
fn approval_resolution_aborts_turn_when_approval_is_aborted() {
    let resolution = ApprovalResolution {
        decision: ReviewDecision::Abort,
        source: ApprovalResolutionSource::User,
    };

    assert!(matches!(
        resolution.into_tool_result(&model_info_from_slug("acting-model")),
        Err(ToolError::Codex(error))
            if matches!(
                error.details(),
                codex_protocol::error::CodexErrorDetails::TurnAborted
            )
    ));
}

#[test]
fn approval_resolution_uses_acting_model_timeout_instructions() {
    let mut model = model_info_from_slug("acting-model");
    for timeout_instructions in ["Catalog timeout instructions.", ""] {
        model.model_messages = Some(
            serde_json::from_value(serde_json::json!({
                "auto_review": {
                    "timeout_instructions": timeout_instructions,
                },
            }))
            .expect("model messages should deserialize"),
        );
        let resolution = ApprovalResolution {
            decision: ReviewDecision::TimedOut,
            source: ApprovalResolutionSource::Guardian,
        };

        assert!(matches!(
            resolution.into_tool_result(&model),
            Err(ToolError::Rejected(rejection)) if rejection == timeout_instructions
        ));
    }
}

#[test]
fn guardian_cwd_preserves_drive_shaped_local_posix_path() {
    let native_cwd = AbsolutePathBuf::try_from(std::path::PathBuf::from("/C:/workspace"))
        .expect("drive-shaped POSIX path should be absolute");
    let cwd = PathUri::from_abs_path(&native_cwd);

    assert_eq!(
        guardian_cwd(codex_exec_server::LOCAL_ENVIRONMENT_ID, cwd)
            .expect("local cwd should retain the host path convention"),
        native_cwd
    );
}

#[test]
fn guardian_cwd_rejects_foreign_remote_path() {
    let cwd = PathUri::parse("file:///C:/workspace").expect("valid Windows path URI");

    assert!(guardian_cwd(codex_exec_server::REMOTE_ENVIRONMENT_ID, cwd).is_err());
}
