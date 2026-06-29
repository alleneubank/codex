use std::sync::Arc;

use crate::agents_md::LoadedAgentsMd;
use crate::environment_selection::TurnEnvironmentSnapshot;
use crate::session::McpRuntimeSnapshot;
use crate::session::turn_context::TurnContext;
use codex_exec_server::ResolvedSelectedCapabilityRoot;
use codex_utils_absolute_path::AbsolutePathBuf;

/// Request-scoped state that may change between model sampling requests.
#[derive(Debug)]
pub(crate) struct StepContext {
    pub(crate) turn: Arc<TurnContext>,
    pub(crate) environments: TurnEnvironmentSnapshot,
    /// Workspace roots from the same session configuration snapshot as this step.
    pub(crate) workspace_roots: Vec<AbsolutePathBuf>,
    /// Capability roots bound to ready environments in this exact step.
    pub(crate) selected_capability_roots: Vec<ResolvedSelectedCapabilityRoot>,
    /// The exact MCP config and manager used to advertise and execute tools for this step.
    pub(crate) mcp: Arc<McpRuntimeSnapshot>,
    /// The canonical AGENTS.md value observed with this environment snapshot.
    pub(crate) loaded_agents_md: Option<Arc<LoadedAgentsMd>>,
}

impl StepContext {
    pub(crate) fn new(
        turn: Arc<TurnContext>,
        environments: TurnEnvironmentSnapshot,
        workspace_roots: Vec<AbsolutePathBuf>,
        selected_capability_roots: Vec<ResolvedSelectedCapabilityRoot>,
        mcp: Arc<McpRuntimeSnapshot>,
        loaded_agents_md: Option<Arc<LoadedAgentsMd>>,
    ) -> Self {
        Self {
            turn,
            environments,
            workspace_roots,
            selected_capability_roots,
            mcp,
            loaded_agents_md,
        }
    }
}
