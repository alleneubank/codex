use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn elicitation_open_notifications_classify_every_request_shape() {
    let form_fields = || (None, "Prompt".to_string(), json!({ "type": "object" }));
    let requests = [
        ElicitationRequest::UserVerification {
            meta: None,
            title: "Verify identity".to_string(),
            description: "Confirm in the app".to_string(),
            challenge: "fixture-challenge".to_string(),
        },
        {
            let (meta, message, requested_schema) = form_fields();
            ElicitationRequest::Form {
                meta,
                message,
                requested_schema,
            }
        },
        {
            let (meta, message, requested_schema) = form_fields();
            ElicitationRequest::OpenAiForm {
                meta,
                message,
                requested_schema,
            }
        },
        {
            let (meta, message, requested_schema) = form_fields();
            ElicitationRequest::OpenAiElicitationForm {
                meta,
                message,
                requested_schema,
            }
        },
        ElicitationRequest::Url {
            meta: None,
            message: "Open URL".to_string(),
            url: "https://example.com".to_string(),
            elicitation_id: "elicitation-1".to_string(),
        },
    ];

    assert_eq!(
        requests.map(|request| elicitation_open_notification_type(&request)),
        [
            None,
            Some(codex_hooks::NotificationType::ElicitationDialog),
            Some(codex_hooks::NotificationType::ElicitationDialog),
            Some(codex_hooks::NotificationType::ElicitationDialog),
            Some(codex_hooks::NotificationType::ElicitationUrlDialog),
        ]
    );
}

#[test]
fn plugin_install_elicitation_telemetry_metadata_requires_install_tool_suggestion() {
    let event = EventMsg::ElicitationRequest(ElicitationRequestEvent {
        turn_id: Some("turn-1".to_string()),
        server_name: "codex_apps".to_string(),
        id: codex_protocol::mcp::RequestId::String("request-1".to_string()),
        request: codex_protocol::approvals::ElicitationRequest::Form {
            meta: Some(json!({
                "codex_approval_kind": "tool_suggestion",
                "suggest_type": "install",
                "tool_type": "plugin",
                "tool_id": "slack@openai-curated",
                "tool_name": "Slack",
            })),
            message: "Install Slack?".to_string(),
            requested_schema: json!({
                "type": "object",
                "properties": {},
            }),
        },
    });

    assert_eq!(
        plugin_install_elicitation_telemetry_metadata(&event),
        Some(PluginInstallElicitationTelemetryMetadata {
            tool_type: "plugin".to_string(),
            tool_id: "slack@openai-curated".to_string(),
            tool_name: "Slack".to_string(),
        })
    );

    let enable_event = EventMsg::ElicitationRequest(ElicitationRequestEvent {
        turn_id: Some("turn-1".to_string()),
        server_name: "codex_apps".to_string(),
        id: codex_protocol::mcp::RequestId::String("request-2".to_string()),
        request: codex_protocol::approvals::ElicitationRequest::Form {
            meta: Some(json!({
                "codex_approval_kind": "tool_suggestion",
                "suggest_type": "enable",
                "tool_type": "plugin",
                "tool_id": "slack@openai-curated",
                "tool_name": "Slack",
            })),
            message: "Enable Slack?".to_string(),
            requested_schema: json!({
                "type": "object",
                "properties": {},
            }),
        },
    });

    assert_eq!(
        plugin_install_elicitation_telemetry_metadata(&enable_event),
        None
    );
}
