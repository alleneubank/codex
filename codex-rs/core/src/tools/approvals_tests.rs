use super::*;
use crate::session::tests::make_session_and_context_with_rx;
use crate::session::turn_context::TurnContext;
use crate::state::ActiveTurn;
use anyhow::Context;
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
    sync_decision: Option<&str>,
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
            "#!/bin/sh\ncat >/dev/null\nprintf 1 >> {}\n",
            shlex::try_quote(marker.to_string_lossy().as_ref()).expect("quote observer marker")
        ),
    )
    .expect("write async observer fixture");

    let mut handlers = Vec::new();
    if let Some(sync_decision) = sync_decision {
        let policy_script = codex_home.join("permission-request-policy.sh");
        std::fs::write(
            &policy_script,
            format!(
                "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{}'\n",
                serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PermissionRequest",
                        "decision": {
                            "behavior": sync_decision,
                            "message": (sync_decision == "deny").then_some("policy denied"),
                        },
                    },
                })
            ),
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
struct FixedApprovalContributor {
    decision: codex_extension_api::ApprovalDecision,
}

#[cfg(unix)]
impl codex_extension_api::ApprovalReviewContributor for FixedApprovalContributor {
    fn decide<'a>(
        &'a self,
        _input: &'a codex_extension_api::ApprovalDecisionInput<'_>,
    ) -> codex_extension_api::ExtensionFuture<'a, Option<codex_extension_api::ApprovalDecision>>
    {
        Box::pin(async move { Some(self.decision.clone()) })
    }
}

#[cfg(unix)]
fn observer_exec_command_action(turn_context: &TurnContext, call_id: &str) -> ApprovalAction {
    ApprovalAction::ExecCommand {
        id: call_id.to_string(),
        environment_id: codex_exec_server::LOCAL_ENVIRONMENT_ID.to_string(),
        command: vec!["printf".to_string(), "approved".to_string()],
        hook_command: "printf approved".to_string(),
        cwd: PathUri::from_abs_path(&turn_context.config.cwd),
        sandbox_permissions: SandboxPermissions::UseDefault,
        additional_permissions: None,
        justification: None,
        tty: false,
        proposed_execpolicy_amendment: None,
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
    let (session, turn_context, events, marker) =
        observed_approval_session(/*sync_decision*/ None).await;
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
        ApprovalAction::WriteStdin {
            id: "write-stdin-route".to_string(),
            approval_id: "write-stdin-route-approval".to_string(),
            environment_id: codex_exec_server::LOCAL_ENVIRONMENT_ID.to_string(),
            process_id: 42,
            input: "echo observed\n".to_string(),
            cwd: cwd_uri.clone(),
            tty: false,
            sandbox_permissions: SandboxPermissions::UseDefault,
            additional_permissions: None,
        },
        ExpectedPromptEvent::Command,
    )
    .await;

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
async fn synchronous_policy_decision_suppresses_coexisting_async_observer() -> anyhow::Result<()> {
    assert_sync_policy_precedes_extension(
        "allow",
        ApprovalResolution {
            decision: ReviewDecision::Approved,
            source: ApprovalResolutionSource::Hook,
        },
    )
    .await
}

#[cfg(unix)]
async fn assert_sync_policy_precedes_extension(
    policy_behavior: &str,
    expected_resolution: ApprovalResolution,
) -> anyhow::Result<()> {
    let (mut session, turn_context, events, marker) =
        observed_approval_session(Some(policy_behavior)).await;
    let mut extensions = codex_extension_api::ExtensionRegistryBuilder::new();
    extensions.approval_review_contributor(Arc::new(FixedApprovalContributor {
        decision: codex_extension_api::ApprovalDecision::Allow,
    }));
    Arc::get_mut(&mut session)
        .context("approval fixture session must be uniquely owned")?
        .services
        .extensions = Arc::new(extensions.build());

    let action = observer_exec_command_action(&turn_context, "policy-route");
    let resolution = session
        .request_reviewer_approval(
            action,
            &observer_approval_context(turn_context, "policy-route"),
        )
        .await;
    assert_eq!(resolution, expected_resolution);
    session.hooks().wait_for_async_hooks().await;
    assert!(!marker.exists());
    assert!(events.try_recv().is_err());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn synchronous_policy_deny_precedes_extension_and_observer() -> anyhow::Result<()> {
    assert_sync_policy_precedes_extension(
        "deny",
        ApprovalResolution {
            decision: ReviewDecision::denied("policy denied"),
            source: ApprovalResolutionSource::Hook,
        },
    )
    .await
}

#[cfg(unix)]
#[tokio::test]
async fn extension_allow_skips_human_prompt_observer_and_permission_events() {
    let (mut session, turn_context, events, marker) =
        observed_approval_session(/*sync_decision*/ None).await;
    let mut extensions = codex_extension_api::ExtensionRegistryBuilder::new();
    extensions.approval_review_contributor(Arc::new(FixedApprovalContributor {
        decision: codex_extension_api::ApprovalDecision::Allow,
    }));
    Arc::get_mut(&mut session)
        .expect("session is uniquely owned")
        .services
        .extensions = Arc::new(extensions.build());

    let resolution = session
        .request_reviewer_approval(
            observer_exec_command_action(&turn_context, "extension-allow"),
            &observer_approval_context(turn_context, "extension-allow"),
        )
        .await;
    assert_eq!(
        resolution,
        ApprovalResolution {
            decision: ReviewDecision::Approved,
            source: ApprovalResolutionSource::Guardian,
        }
    );
    session.hooks().wait_for_async_hooks().await;
    assert!(!marker.exists());
    assert!(events.try_recv().is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn extension_ask_user_prompts_once_and_runs_one_observer() {
    let (mut session, turn_context, events, marker) =
        observed_approval_session(/*sync_decision*/ None).await;
    let mut extensions = codex_extension_api::ExtensionRegistryBuilder::new();
    extensions.approval_review_contributor(Arc::new(FixedApprovalContributor {
        decision: codex_extension_api::ApprovalDecision::AskUser,
    }));
    Arc::get_mut(&mut session)
        .expect("session is uniquely owned")
        .services
        .extensions = Arc::new(extensions.build());

    let approval_session = Arc::clone(&session);
    let approval_context = observer_approval_context(Arc::clone(&turn_context), "extension-user");
    let approval_task = tokio::spawn(async move {
        approval_session
            .request_reviewer_approval(
                observer_exec_command_action(&turn_context, "extension-user"),
                &approval_context,
            )
            .await
    });
    let approval_request = timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.expect("approval event stream");
            match event.msg {
                EventMsg::ExecApprovalRequest(request) => break request,
                EventMsg::HookStarted(event)
                    if event.run.event_name
                        == codex_protocol::protocol::HookEventName::PermissionRequest =>
                {
                    panic!("extension AskUser must not emit PermissionRequest hook events")
                }
                EventMsg::HookCompleted(event)
                    if event.run.event_name
                        == codex_protocol::protocol::HookEventName::PermissionRequest =>
                {
                    panic!("extension AskUser must not emit PermissionRequest hook events")
                }
                _ => {}
            }
        }
    })
    .await
    .expect("extension AskUser should expose a human approval prompt");
    session
        .notify_approval(&approval_request.call_id, ReviewDecision::Approved)
        .await;
    let resolution = approval_task.await.expect("approval task should finish");
    session.hooks().wait_for_async_hooks().await;
    assert_eq!(
        resolution,
        ApprovalResolution {
            decision: ReviewDecision::Approved,
            source: ApprovalResolutionSource::User,
        }
    );
    assert_eq!(
        std::fs::read_to_string(marker).expect("observer marker"),
        "1"
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

#[cfg(unix)]
#[test_case::test_case(ApprovalsReviewer::User, codex_extension_api::ApprovalDecision::AskUser; "manual prompt")]
#[test_case::test_case(ApprovalsReviewer::AutoReview, codex_extension_api::ApprovalDecision::Allow; "cached allow")]
#[tokio::test]
async fn non_utf8_cwd_preserves_approval_routing(
    reviewer: ApprovalsReviewer,
    decision: codex_extension_api::ApprovalDecision,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use codex_extension_api::ApprovalDecision;
    use std::os::unix::ffi::OsStringExt;

    struct Contributor {
        cwd: codex_utils_path_uri::LegacyAppPathString,
        decision: ApprovalDecision,
    }

    impl codex_extension_api::ApprovalReviewContributor for Contributor {
        fn decide<'a>(
            &'a self,
            input: &'a codex_extension_api::ApprovalDecisionInput<'_>,
        ) -> codex_extension_api::ExtensionFuture<'a, Option<ApprovalDecision>> {
            Box::pin(async move {
                assert_eq!(input.action["cwd"], serde_json::json!(self.cwd));
                Some(self.decision.clone())
            })
        }
    }

    let cwd = PathUri::from_abs_path(&AbsolutePathBuf::try_from(PathBuf::from(
        std::ffi::OsString::from_vec(b"/tmp/non-utf8-\xe9".to_vec()),
    ))?);
    let (mut session, turn, events) = make_session_and_context_with_rx().await;
    let mut extensions = codex_extension_api::ExtensionRegistryBuilder::new();
    extensions.approval_review_contributor(Arc::new(Contributor {
        cwd: codex_utils_path_uri::LegacyAppPathString::from_path_uri(&cwd, PathConvention::Posix)?,
        decision,
    }));
    Arc::get_mut(&mut session)
        .context("session is uniquely owned")?
        .services
        .extensions = Arc::new(extensions.build());
    *session.active_turn.lock().await = Some(crate::state::ActiveTurn::default());
    let mut review_context = GuardianReviewContext::from(&turn);
    review_context.approval_policy = AskForApproval::OnRequest;
    review_context.approvals_reviewer = reviewer;
    let context = ApprovalContext {
        review_context,
        cancellation_token: None,
        call_id: "non-utf8-cwd".to_string(),
        tool_name: ToolName::plain("exec_command"),
        strict_auto_review: false,
        approval_reason: None,
        retry_reason: None,
        network_approval_context: None,
    };
    let action = ApprovalAction::ExecCommand {
        id: context.call_id.clone(),
        environment_id: codex_exec_server::LOCAL_ENVIRONMENT_ID.to_string(),
        command: vec!["npm".to_string(), "install".to_string()],
        hook_command: "npm install".to_string(),
        cwd: cwd.clone(),
        sandbox_permissions: if reviewer == ApprovalsReviewer::User {
            SandboxPermissions::RequireEscalated
        } else {
            SandboxPermissions::UseDefault
        },
        additional_permissions: None,
        justification: None,
        tty: false,
        proposed_execpolicy_amendment: None,
    };
    let approval = session.request_reviewer_approval(action, &context);
    tokio::pin!(approval);
    let expected = if reviewer == ApprovalsReviewer::User {
        tokio::select! {
            resolution = &mut approval => panic!("expected a user prompt, got {resolution:?}"),
            event = events.recv() => {
                let codex_protocol::protocol::EventMsg::ExecApprovalRequest(request) =
                    event.context("receive user prompt")?.msg
                else {
                    panic!("expected a command approval prompt");
                };
                assert_eq!(request.cwd, codex_utils_path_uri::LegacyAppPathString::from(cwd));
                assert_eq!(request.command, vec!["npm", "install"]);
                session.notify_approval(&request.call_id, ReviewDecision::Approved).await;
            }
        }
        ApprovalResolution {
            decision: ReviewDecision::Approved,
            source: ApprovalResolutionSource::User,
        }
    } else {
        ApprovalResolution {
            decision: ReviewDecision::Approved,
            source: ApprovalResolutionSource::Guardian,
        }
    };
    assert_eq!(approval.await, expected);
    assert!(events.try_recv().is_err());
    Ok(())
}

#[tokio::test]
async fn explicit_mcp_reviewer_override_takes_precedence_over_action_context() {
    let (session, turn, events) = make_session_and_context_with_rx().await;
    let action = ApprovalAction::McpToolCall {
        id: "mcp-override".to_string(),
        server: "example".to_string(),
        tool_name: "dangerous".to_string(),
        arguments: None,
        connector_id: None,
        connector_name: None,
        connector_description: None,
        connected_account_email: None,
        tool_title: None,
        tool_description: None,
        annotations: None,
        hook_tool_name: HookToolName::new("mcp__example__dangerous"),
        approval_policy: AskForApproval::OnRequest,
        reviewer: ApprovalsReviewer::User,
        approval_mode: AppToolApproval::Prompt,
        allow_session_remember: false,
        allow_persistent_approval: false,
    };
    let mut review_context = GuardianReviewContext::from(&turn);
    review_context.approval_policy = AskForApproval::OnRequest;
    review_context.approvals_reviewer = ApprovalsReviewer::AutoReview;
    let context = ApprovalContext {
        review_context,
        cancellation_token: None,
        call_id: "mcp-override".to_string(),
        tool_name: ToolName::plain("dangerous"),
        strict_auto_review: false,
        approval_reason: None,
        retry_reason: None,
        network_approval_context: None,
    };

    tokio::select! {
        resolution = session.request_reviewer_approval(action, &context) => {
            panic!("expected a user approval request, got {resolution:?}");
        }
        event = events.recv() => {
            let codex_protocol::protocol::EventMsg::ElicitationRequest(request) =
                event.expect("receive user approval request").msg
            else {
                panic!("expected an MCP user approval request");
            };
            assert_eq!(request.server_name, "example");
            assert_eq!(
                request.id,
                codex_protocol::mcp::RequestId::String(
                    "mcp_tool_call_approval_mcp-override".to_string()
                )
            );
        }
    }
}
