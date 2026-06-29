use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;

pub(crate) const ENTER_WORKTREE_TOOL_NAME: &str = "enter_worktree";
pub(crate) const EXIT_WORKTREE_TOOL_NAME: &str = "exit_worktree";

#[cfg(test)]
#[path = "worktree_spec_tests.rs"]
mod tests;

pub(crate) fn create_enter_worktree_tool() -> ToolSpec {
    let name_property = JsonSchema::string(Some(
        "Codex-managed worktree name. Required unless `path` is provided. Must not be combined with `path`."
            .to_string(),
    ));
    let path_property = JsonSchema::string(Some(
        "Existing Codex-managed worktree path for the same repository. Relative paths resolve under the current cwd. Required unless `name` is provided. Must not be combined with `name`."
            .to_string(),
    ));
    let mut parameters = JsonSchema::object(
        BTreeMap::from([
            ("name".to_string(), name_property.clone()),
            ("path".to_string(), path_property.clone()),
        ]),
        /*required*/ None,
        Some(false.into()),
    );
    parameters.one_of = Some(vec![
        JsonSchema::object(
            BTreeMap::from([("name".to_string(), name_property)]),
            Some(vec!["name".to_string()]),
            Some(false.into()),
        ),
        JsonSchema::object(
            BTreeMap::from([("path".to_string(), path_property)]),
            Some(vec!["path".to_string()]),
            Some(false.into()),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: ENTER_WORKTREE_TOOL_NAME.to_string(),
        description: concat!(
            "Enter a git worktree for this session. The cwd change is applied to later ",
            "serialized tool calls and subsequent model requests. Provide either `name` ",
            "for a Codex-managed worktree, ",
            "or `path` for an existing Codex-managed worktree in the same repository."
        )
        .to_string(),
        strict: false,
        defer_loading: None,
        parameters,
        output_schema: Some(worktree_output_schema()),
    })
}

pub(crate) fn create_exit_worktree_tool() -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: EXIT_WORKTREE_TOOL_NAME.to_string(),
        description: concat!(
            "Exit the active worktree and restore this session's original cwd. The cwd change ",
            "is applied to later serialized tool calls and subsequent model requests. By default ",
            "this keeps the worktree. Pass `keep: false` to remove the active clean Codex-managed ",
            "worktree after restoring the original cwd; dirty worktrees are not force-removed."
        )
        .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::from([(
                "keep".to_string(),
                JsonSchema::boolean(Some(
                    "Whether to keep the active Codex-managed worktree after exiting. Defaults to true; set to false to remove a clean managed worktree."
                        .to_string(),
                )),
            )]),
            /*required*/ None,
            Some(false.into()),
        ),
        output_schema: Some(exit_worktree_output_schema()),
    })
}

fn worktree_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "cwd": { "type": "string" },
            "worktree_path": { "type": "string" },
            "original_cwd": { "type": "string" },
            "branch": {
                "anyOf": [
                    { "type": "string" },
                    { "type": "null" }
                ]
            },
            "name": {
                "anyOf": [
                    { "type": "string" },
                    { "type": "null" }
                ]
            },
            "created": {
                "anyOf": [
                    { "type": "boolean" },
                    { "type": "null" }
                ]
            }
        },
        "required": ["cwd", "worktree_path", "original_cwd", "branch", "name", "created"],
        "additionalProperties": false
    })
}

fn exit_worktree_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "cwd": { "type": "string" },
            "worktree_path": { "type": "string" },
            "original_cwd": { "type": "string" },
            "branch": {
                "anyOf": [
                    { "type": "string" },
                    { "type": "null" }
                ]
            },
            "name": {
                "anyOf": [
                    { "type": "string" },
                    { "type": "null" }
                ]
            },
            "created": {
                "anyOf": [
                    { "type": "boolean" },
                    { "type": "null" }
                ]
            },
            "keep": { "type": "boolean" },
            "removed": { "type": "boolean" }
        },
        "required": [
            "cwd",
            "worktree_path",
            "original_cwd",
            "branch",
            "name",
            "created",
            "keep",
            "removed"
        ],
        "additionalProperties": false
    })
}
