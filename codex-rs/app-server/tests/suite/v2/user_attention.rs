use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::create_mock_responses_server_repeating_assistant;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadArchiveParams;
use codex_app_server_protocol::ThreadArchiveResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::UserInput;
use codex_features::Feature;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::MockServer;

const READ_TIMEOUT: Duration = Duration::from_secs(20);
const PRIVATE_PLAN_TEXT: &str = "private plan text must not reach attention hooks";

struct AttentionFixture {
    app_server: TestAppServer,
    thread_id: String,
    turn_id: String,
    log_path: PathBuf,
    _codex_home: TempDir,
    _responses_server: MockServer,
}

#[tokio::test]
async fn concurrent_attention_ids_reject_duplicates_and_complete_idempotently() -> Result<()> {
    let mut fixture = attention_fixture().await?;

    let first_id = fixture.send_start("plan-prompt-1").await?;
    let second_id = fixture.send_start("plan-prompt-2").await?;
    assert_eq!(fixture.read_success(first_id).await?, json!({}));
    assert_eq!(fixture.read_success(second_id).await?, json!({}));

    let duplicate_id = fixture.send_start("plan-prompt-1").await?;
    let duplicate = timeout(
        READ_TIMEOUT,
        fixture
            .app_server
            .read_stream_until_error_message(RequestId::Integer(duplicate_id)),
    )
    .await??;
    assert_eq!(duplicate.error.code, -32600);
    assert_eq!(duplicate.error.message, "attentionId is already active");

    assert_eq!(fixture.complete("plan-prompt-1").await?, json!({}));
    assert_eq!(fixture.complete("plan-prompt-1").await?, json!({}));
    assert_eq!(fixture.complete("plan-prompt-2").await?, json!({}));

    wait_for_payload_count(&fixture.log_path, /*count*/ 4).await?;
    let payloads = read_payloads(&fixture.log_path)?;
    assert_eq!(
        payloads
            .iter()
            .map(|payload| payload["notification_type"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!("plan_implementation_request"),
            json!("plan_implementation_request"),
            json!("plan_implementation_complete"),
            json!("plan_implementation_complete"),
        ]
    );
    for payload in &payloads {
        let serialized = payload.to_string();
        assert!(!serialized.contains(PRIVATE_PLAN_TEXT));
        assert!(!serialized.contains("plan-prompt-1"));
        assert!(!serialized.contains("plan-prompt-2"));
    }
    Ok(())
}

#[tokio::test]
async fn start_rejects_a_turn_that_has_not_completed() -> Result<()> {
    let mut fixture = attention_fixture().await?;
    let request_id = fixture
        .app_server
        .send_raw_request(
            "thread/userAttention/start",
            Some(json!({
                "threadId": fixture.thread_id,
                "turnId": "not-a-completed-turn",
                "attentionId": "invalid-turn",
                "kind": "planImplementation",
            })),
        )
        .await?;
    let error = timeout(
        READ_TIMEOUT,
        fixture
            .app_server
            .read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, -32600);
    assert_eq!(
        error.error.message,
        "completed turn hook context is unavailable"
    );
    assert_eq!(read_payloads(&fixture.log_path)?, Vec::<Value>::new());
    Ok(())
}

#[tokio::test]
async fn thread_teardown_completes_owned_attention_exactly_once() -> Result<()> {
    let mut fixture = attention_fixture().await?;
    let start_id = fixture.send_start("archive-prompt").await?;
    assert_eq!(fixture.read_success(start_id).await?, json!({}));

    let archive_id = fixture
        .app_server
        .send_thread_archive_request(ThreadArchiveParams {
            thread_id: fixture.thread_id.clone(),
        })
        .await?;
    let _: ThreadArchiveResponse =
        timeout(READ_TIMEOUT, fixture.app_server.read_response(archive_id)).await??;
    assert_eq!(fixture.complete("archive-prompt").await?, json!({}));

    wait_for_payload_count(&fixture.log_path, /*count*/ 2).await?;
    assert_eq!(
        read_payloads(&fixture.log_path)?
            .into_iter()
            .map(|payload| payload["notification_type"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!("plan_implementation_request"),
            json!("plan_implementation_complete"),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn connection_disconnect_completes_owned_attention_exactly_once() -> Result<()> {
    let mut fixture = attention_fixture().await?;
    let start_id = fixture.send_start("disconnect-prompt").await?;
    assert_eq!(fixture.read_success(start_id).await?, json!({}));

    let status = timeout(READ_TIMEOUT, fixture.app_server.shutdown_gracefully()).await??;
    assert!(status.success(), "app-server did not exit successfully");

    wait_for_payload_count(&fixture.log_path, /*count*/ 2).await?;
    assert_eq!(
        read_payloads(&fixture.log_path)?
            .into_iter()
            .map(|payload| payload["notification_type"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!("plan_implementation_request"),
            json!("plan_implementation_complete"),
        ]
    );
    Ok(())
}

impl AttentionFixture {
    async fn send_start(&mut self, attention_id: &str) -> Result<i64> {
        self.app_server
            .send_raw_request(
                "thread/userAttention/start",
                Some(json!({
                    "threadId": self.thread_id,
                    "turnId": self.turn_id,
                    "attentionId": attention_id,
                    "kind": "planImplementation",
                })),
            )
            .await
    }

    async fn read_success(&mut self, request_id: i64) -> Result<Value> {
        timeout(READ_TIMEOUT, self.app_server.read_response(request_id)).await?
    }

    async fn complete(&mut self, attention_id: &str) -> Result<Value> {
        let request_id = self
            .app_server
            .send_raw_request(
                "thread/userAttention/complete",
                Some(json!({
                    "threadId": self.thread_id,
                    "attentionId": attention_id,
                })),
            )
            .await?;
        self.read_success(request_id).await
    }
}

async fn attention_fixture() -> Result<AttentionFixture> {
    let responses_server = create_mock_responses_server_repeating_assistant("Plan ready").await;
    let codex_home = TempDir::new()?;
    let log_path = write_config_and_hook(codex_home.path(), &responses_server.uri())?;
    let mut app_server = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build_initialized_with_timeout(READ_TIMEOUT)
        .await?;
    let ThreadStartResponse { thread, .. } = app_server
        .start_thread(ThreadStartParams {
            model: Some("mock-model".to_string()),
            config: Some(HashMap::from([(
                "bypass_hook_trust".to_string(),
                json!(true),
            )])),
            ..Default::default()
        })
        .await?;
    let completed = app_server
        .start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: PRIVATE_PLAN_TEXT.to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    Ok(AttentionFixture {
        app_server,
        thread_id: thread.id,
        turn_id: completed.turn.id,
        log_path,
        _codex_home: codex_home,
        _responses_server: responses_server,
    })
}

fn write_config_and_hook(codex_home: &Path, server_uri: &str) -> Result<PathBuf> {
    let log_path = codex_home.join("plan-attention.jsonl");
    let script_path = codex_home.join("plan-attention.py");
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
    MockResponsesConfig::new(server_uri)
        .enable_feature(Feature::CodexHooks)
        .with_extra_config(&format!(
            r#"[[hooks.Notification]]
matcher = "plan_implementation_request|plan_implementation_complete"

[[hooks.Notification.hooks]]
type = "command"
command = "python3 {script_path}"
timeout = 3
"#,
            script_path = script_path.display(),
        ))
        .write(codex_home)?;
    Ok(log_path)
}

async fn wait_for_payload_count(log_path: &Path, count: usize) -> Result<()> {
    timeout(READ_TIMEOUT, async {
        loop {
            if read_payloads(log_path).unwrap_or_default().len() >= count {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

fn read_payloads(log_path: &Path) -> Result<Vec<Value>> {
    Ok(fs::read_to_string(log_path)
        .unwrap_or_default()
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<_>, _>>()?)
}
