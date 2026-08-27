use crate::JsonSchema;
use crate::TS;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum UserAttentionKind {
    /// A post-plan decision asking whether Codex should implement the completed plan.
    PlanImplementation,
}

/// Starts one client-owned synchronous user-attention lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadUserAttentionStartParams {
    /// Thread that owns the completed turn and attention lifecycle.
    pub thread_id: String,
    /// Latest completed turn whose hook context should be used.
    pub turn_id: String,
    /// Opaque client-generated correlation ID.
    pub attention_id: String,
    pub kind: UserAttentionKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadUserAttentionStartResponse {}

/// Completes one previously started user-attention lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadUserAttentionCompleteParams {
    pub thread_id: String,
    /// Opaque correlation ID supplied to the matching start request.
    pub attention_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadUserAttentionCompleteResponse {}
