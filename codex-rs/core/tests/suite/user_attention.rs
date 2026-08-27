use std::fs;
use std::path::Path;
use std::time::Duration;

use codex_core::TurnInputRequest;
use codex_features::Feature;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ThreadSettingsOverrides;
use codex_protocol::user_input::UserInput;
use core_test_support::TempDirExt;
use core_test_support::responses;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::local_selections;
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
                environments: Some(local_selections(cwd.abs())),
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
    let script_path = home.join("plan-attention.py");
    let log_path = home.join("plan-attention.jsonl");
    fs::write(
        &script_path,
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
