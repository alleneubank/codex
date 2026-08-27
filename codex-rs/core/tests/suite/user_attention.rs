use std::fs;
use std::path::Path;
use std::time::Duration;

use codex_core::TurnInputRequest;
use codex_features::Feature;
use codex_protocol::protocol::EnvironmentConfigState;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::HookEventName;
use codex_protocol::protocol::HookRunStatus;
use codex_protocol::protocol::ThreadSettingsOverrides;
use codex_protocol::protocol::TurnEnvironmentRequest;
use codex_protocol::protocol::TurnEnvironmentRequests;
use codex_protocol::user_input::UserInput;
use codex_utils_path_uri::PathUri;
use core_test_support::TempDirExt;
use core_test_support::responses;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::skip_if_remote;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::environment_config_for_selection;
use core_test_support::test_codex::local_requests;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tokio::time::timeout;

const PRIVATE_PLAN_TEXT: &str = "private plan text must not reach the attention hook";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plan_implementation_attention_completes_exactly_once_on_complete_and_drop()
-> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    responses::mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("message-1", "Plan ready"),
            ev_completed("response-1"),
        ]),
    )
    .await;
    let TestCodex {
        codex, cwd, home, ..
    } = test_codex()
        .with_pre_build_hook(|home| {
            write_attention_hook(home).expect("write attention hook fixture");
        })
        .with_config(|config| {
            config
                .features
                .enable(Feature::CodexHooks)
                .expect("test config should allow feature update");
            config.bypass_hook_trust = true;
        })
        .build_with_auto_env(&server)
        .await?;

    codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: PRIVATE_PLAN_TEXT.to_string(),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                environments: Some(local_requests(cwd.abs())),
                ..Default::default()
            }),
        )
        .await?;
    let completed =
        wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;
    let EventMsg::TurnComplete(completed) = completed else {
        unreachable!("waited for turn completion");
    };

    codex
        .start_plan_implementation_attention(&completed.turn_id)
        .await?
        .complete()
        .await;
    let cancelled = codex
        .start_plan_implementation_attention(&completed.turn_id)
        .await?;
    drop(cancelled);

    let log_path = home.path().join("plan-attention.jsonl");
    wait_for_payload_count(&log_path, /*count*/ 4).await?;
    let payloads = read_payloads(&log_path)?;
    assert_eq!(
        payloads
            .iter()
            .map(|payload| payload["notification_type"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!("plan_implementation_request"),
            json!("plan_implementation_complete"),
            json!("plan_implementation_request"),
            json!("plan_implementation_complete"),
        ]
    );
    assert_eq!(
        payloads
            .iter()
            .map(|payload| payload["message"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!("Codex needs your input"),
            json!("Codex input request completed"),
            json!("Codex needs your input"),
            json!("Codex input request completed"),
        ]
    );
    assert!(
        payloads
            .iter()
            .all(|payload| !payload.to_string().contains(PRIVATE_PLAN_TEXT))
    );
    Ok(())
}

fn write_attention_hook(home: &Path) -> anyhow::Result<()> {
    write_attention_script(home)?;
    let script_path = home.join("plan-attention.py");
    fs::write(
        home.join("hooks.json"),
        json!({
            "hooks": {
                "Notification": [{
                    "matcher": "plan_implementation_request|plan_implementation_complete",
                    "hooks": [{
                        "type": "command",
                        "command": format!("python3 {}", script_path.display()),
                    }]
                }]
            }
        })
        .to_string(),
    )?;
    Ok(())
}

fn write_attention_script(home: &Path) -> anyhow::Result<()> {
    let log_path = home.join("plan-attention.jsonl");
    fs::write(
        home.join("plan-attention.py"),
        format!(
            r#"import json
from pathlib import Path
import sys

payload = json.load(sys.stdin)
with Path(r"{log_path}").open("a", encoding="utf-8") as handle:
    handle.write(json.dumps(payload) + "\n")
"#,
            log_path = log_path.display(),
        ),
    )?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn environment_plan_attention_runs_in_its_owner_after_turn_completion() -> anyhow::Result<()>
{
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "the fixture observes native filesystem paths and owner cwd"
    );

    let server = start_mock_server().await;
    responses::mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("message-1", "Plan ready"),
            ev_completed("response-1"),
        ]),
    )
    .await;
    let test = test_codex()
        .with_pre_build_hook(|home| {
            write_attention_script(home).expect("write attention script");
        })
        .with_config(|config| {
            config
                .features
                .enable(Feature::CodexHooks)
                .expect("enable hooks");
        })
        .build_with_auto_env(&server)
        .await?;
    let owner_cwd = test.cwd.path().join("hook-owner");
    fs::create_dir(&owner_cwd)?;
    let cwd_log = test.home.path().join("hook-cwd.log");
    let script = test.home.path().join("plan-attention.py");
    let command = format!(
        "python3 {} && pwd -P >> {}",
        shlex::try_quote(script.to_string_lossy().as_ref())?,
        shlex::try_quote(cwd_log.to_string_lossy().as_ref())?,
    );
    let mut selection = test.executor_environment().selection().clone();
    selection.cwd = PathUri::from_host_native_path(&owner_cwd)?;
    let mut config = environment_config_for_selection(&test.config, &selection);
    config.environment_hooks = serde_json::from_value(json!({
        "Notification": [{
            "matcher": "plan_implementation_request|plan_implementation_complete",
            "hooks": [{"type": "command", "command": command}],
        }],
    }))?;
    let request = TurnEnvironmentRequest {
        environment_id: selection.environment_id,
        cwd: selection.cwd,
        workspace_roots: selection.workspace_roots,
        config: EnvironmentConfigState::Ready(config),
    };
    test.codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: PRIVATE_PLAN_TEXT.to_string(),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                environments: Some(TurnEnvironmentRequests::new(test.cwd.abs(), vec![request])),
                ..Default::default()
            }),
        )
        .await?;
    let completed = wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let EventMsg::TurnComplete(completed) = completed else {
        unreachable!("waited for turn completion")
    };
    let log_path = test.home.path().join("plan-attention.jsonl");
    for (index, complete) in [true, false].into_iter().enumerate() {
        let lifecycle = test
            .codex
            .start_plan_implementation_attention(&completed.turn_id)
            .await?;
        if complete {
            lifecycle.complete().await;
        } else {
            drop(lifecycle);
        }
        let count = (index + 1) * 2;
        wait_for_payload_count(&log_path, count).await?;
        let payloads = read_payloads(&log_path)?;
        assert_eq!(payloads.len(), count);
        for kind in [
            "plan_implementation_request",
            "plan_implementation_complete",
        ] {
            assert_eq!(
                payloads
                    .iter()
                    .filter(|payload| payload["notification_type"] == kind)
                    .count(),
                index + 1
            );
        }
        assert!(
            payloads
                .iter()
                .all(|payload| !payload.to_string().contains(PRIVATE_PLAN_TEXT))
        );
    }
    for _ in 0..4 {
        let completed = wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::HookCompleted(completed) if completed.run.event_name == HookEventName::Notification)
        }).await;
        let EventMsg::HookCompleted(completed) = completed else {
            unreachable!("waited for hook completion")
        };
        assert_eq!(completed.run.status, HookRunStatus::Completed);
    }
    // The owner differs from the thread's fallback directory, so this observes executor routing.
    let expected_cwd = owner_cwd.canonicalize()?;
    let execution_cwds = fs::read_to_string(cwd_log)?;
    assert_eq!(execution_cwds.lines().count(), 4);
    assert!(
        execution_cwds
            .lines()
            .all(|cwd| Path::new(cwd) == expected_cwd)
    );
    Ok(())
}

async fn wait_for_payload_count(log_path: &Path, count: usize) -> anyhow::Result<()> {
    if timeout(Duration::from_secs(5), async {
        loop {
            if read_payloads(log_path).unwrap_or_default().len() >= count {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_err()
    {
        let payloads = read_payloads(log_path)?;
        anyhow::bail!(
            "timed out waiting for {count} attention hook payloads; observed {}: {payloads:?}",
            payloads.len()
        );
    }
    Ok(())
}

fn read_payloads(log_path: &Path) -> anyhow::Result<Vec<Value>> {
    Ok(fs::read_to_string(log_path)
        .unwrap_or_default()
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<_>, _>>()?)
}
