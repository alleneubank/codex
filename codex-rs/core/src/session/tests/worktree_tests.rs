use super::*;
use crate::config::PermissionProfileSnapshot;
use crate::environment_selection::EnvironmentConfigOrigin;
use crate::environment_selection::ThreadEnvironments;
use codex_protocol::protocol::EnvironmentConfig;

fn set_turn_permission_profile(
    turn_context: &mut Arc<TurnContext>,
    permission_profile: PermissionProfile,
) {
    let turn_context = Arc::get_mut(turn_context).expect("single turn context ref");
    Arc::make_mut(&mut turn_context.config)
        .permissions
        .set_permission_profile(permission_profile.clone())
        .expect("test permission profile should be allowed");
    let Some(environment) = turn_context
        .environments
        .environments
        .iter_mut()
        .find_map(|state| match state {
            TurnEnvironmentState::Ready(environment)
                if environment.selection.environment_id == LOCAL_ENVIRONMENT_ID =>
            {
                Some(environment)
            }
            _ => None,
        })
    else {
        return;
    };
    environment.config_mut().permission_profile =
        PermissionProfileSnapshot::legacy(permission_profile);
}

fn writable_environment_config() -> EnvironmentConfig {
    EnvironmentConfig {
        allow_login_shell: true,
        workspace_roots: Vec::new(),
        permission_profile: PermissionProfileSnapshot::legacy(PermissionProfile::workspace_write()),
        shell_environment_policy: Default::default(),
        windows_sandbox_level: Default::default(),
        windows_sandbox_private_desktop: false,
        use_legacy_landlock: false,
        exec_policy: None,
        mcp_policy: None,
        network_policy: None,
        selected_capability_roots: Vec::new(),
    }
}

fn set_project_trust_config(
    config: &mut Config,
    projects: impl IntoIterator<Item = (String, TrustLevel)>,
    project_root_markers: Option<Vec<String>>,
    active_trust: TrustLevel,
) {
    set_project_config_entries(
        config,
        projects
            .into_iter()
            .map(|(project, trust_level)| (project, Some(trust_level))),
        project_root_markers,
        active_trust,
    );
}

fn set_project_config_entries(
    config: &mut Config,
    projects: impl IntoIterator<Item = (String, Option<TrustLevel>)>,
    project_root_markers: Option<Vec<String>>,
    active_trust: TrustLevel,
) {
    let config_toml = ConfigToml {
        projects: Some(
            projects
                .into_iter()
                .map(|(project, trust_level)| (project, ProjectConfig { trust_level }))
                .collect(),
        ),
        project_root_markers,
        ..Default::default()
    };
    let user_config_file = config.codex_home.join(codex_config::CONFIG_TOML_FILE);
    std::fs::write(
        &user_config_file,
        toml::to_string(&config_toml).expect("serialize test config file"),
    )
    .expect("write test user config");
    config.config_layer_stack = config
        .config_layer_stack
        .with_user_config(
            &user_config_file,
            toml::Value::try_from(config_toml).expect("serialize test config"),
        )
        .expect("install test user config");
    config.active_project = ProjectConfig {
        trust_level: Some(active_trust),
    };
}

fn init_worktree_tool_repo(repo_path: &Path) -> anyhow::Result<()> {
    run_worktree_tool_git(repo_path, &["init"])?;
    run_worktree_tool_git(repo_path, &["checkout", "-B", "main"])?;
    run_worktree_tool_git(repo_path, &["config", "core.autocrlf", "false"])?;
    std::fs::write(repo_path.join("README.md"), "hello\n")?;
    run_worktree_tool_git(repo_path, &["add", "README.md"])?;
    run_worktree_tool_git(
        repo_path,
        &[
            "-c",
            "user.name=Tester",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial commit",
        ],
    )?;
    Ok(())
}

fn run_worktree_tool_git(repo_path: &Path, args: &[&str]) -> anyhow::Result<()> {
    let output = Command::new("git")
        .current_dir(repo_path)
        .args(args)
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed in {}: stdout={} stderr={}",
            args,
            repo_path.display(),
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

async fn make_worktree_tool_session(
    repo_path: &Path,
) -> anyhow::Result<(Arc<Session>, Arc<TurnContext>)> {
    let (session, turn_context, _rx) =
        make_worktree_tool_session_with_config(repo_path, |_| {}).await?;
    Ok((session, turn_context))
}

async fn make_worktree_tool_session_with_rx(
    repo_path: &Path,
) -> anyhow::Result<(
    Arc<Session>,
    Arc<TurnContext>,
    async_channel::Receiver<Event>,
)> {
    make_worktree_tool_session_with_config(repo_path, |_| {}).await
}

async fn make_worktree_tool_session_with_config(
    repo_path: &Path,
    config_mutator: impl FnOnce(&mut Config),
) -> anyhow::Result<(
    Arc<Session>,
    Arc<TurnContext>,
    async_channel::Receiver<Event>,
)> {
    let repo_path = repo_path.abs();
    let environment_manager = Arc::new(
        codex_exec_server::EnvironmentManager::create_for_tests_with_local(
            Some("ws://127.0.0.1:1".to_string()),
            ExecServerRuntimePaths::new(
                std::env::current_exe()?,
                /*codex_linux_sandbox_exe*/ None,
            )?,
        )
        .await,
    );
    let original_cwd = PathUri::from_abs_path(&repo_path);
    let remote_selection = TurnEnvironmentSelection {
        environment_id: REMOTE_ENVIRONMENT_ID.to_string(),
        cwd: original_cwd.clone(),
        workspace_roots: vec![original_cwd.clone()],
        config: EnvironmentConfigState::FromThread,
    };
    let remote_environment = environment_manager
        .get_environment(REMOTE_ENVIRONMENT_ID)
        .expect("registered remote environment");
    let inherited_environments = TurnEnvironmentSnapshot {
        environments: vec![TurnEnvironmentState::Ready(TurnEnvironment::new(
            TurnEnvironmentSelection {
                config: EnvironmentConfigState::Ready(writable_environment_config()),
                ..remote_selection.clone()
            },
            EnvironmentConfigOrigin::Thread,
            remote_environment,
            /*shell*/ None,
        ))],
    };
    let config_cwd = repo_path.clone();
    let (session, rx) = make_session_with_config_and_rx_with_environments(
        move |config| {
            config.cwd = config_cwd;
            config_mutator(config);
        },
        Some(vec![local(repo_path), remote_selection]),
        Some(inherited_environments),
        Some(environment_manager),
    )
    .await?;
    let turn_context = session.new_default_turn().await;
    Ok((session, turn_context, rx))
}

fn worktree_tool_invocation(
    session: Arc<Session>,
    turn_context: Arc<TurnContext>,
    tool_name: &str,
    arguments: serde_json::Value,
) -> ToolInvocation {
    ToolInvocation {
        session,
        step_context: StepContext::for_test(Arc::clone(&turn_context)),
        turn: turn_context,
        cancellation_token: CancellationToken::new(),
        tracker: Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new())),
        call_id: format!("{tool_name}-call"),
        tool_name: codex_tools::ToolName::plain(tool_name),
        source: ToolCallSource::Direct,
        payload: ToolPayload::Function {
            arguments: arguments.to_string(),
        },
    }
}

async fn enter_worktree_result(
    session: Arc<Session>,
    turn_context: Arc<TurnContext>,
    arguments: serde_json::Value,
) -> std::result::Result<(), FunctionCallError> {
    EnterWorktreeHandler
        .handle(worktree_tool_invocation(
            session,
            turn_context,
            "enter_worktree",
            arguments,
        ))
        .await
        .map(|_| ())
}

async fn exit_worktree_result(
    session: Arc<Session>,
    turn_context: Arc<TurnContext>,
) -> std::result::Result<(), FunctionCallError> {
    exit_worktree_result_with_arguments(session, turn_context, json!({})).await
}

async fn exit_worktree_result_with_arguments(
    session: Arc<Session>,
    turn_context: Arc<TurnContext>,
    arguments: serde_json::Value,
) -> std::result::Result<(), FunctionCallError> {
    ExitWorktreeHandler
        .handle(worktree_tool_invocation(
            session,
            turn_context,
            "exit_worktree",
            arguments,
        ))
        .await
        .map(|_| ())
}

async fn expect_thread_settings_applied(rx: &async_channel::Receiver<Event>) {
    loop {
        let event = timeout(Duration::from_secs(/*secs*/ 5), rx.recv())
            .await
            .expect("timed out waiting for ThreadSettingsApplied")
            .expect("event stream should stay open");
        if matches!(
            event.msg,
            codex_protocol::protocol::EventMsg::ThreadSettingsApplied(_)
        ) {
            return;
        }
    }
}

fn assert_respond_to_model(result: std::result::Result<(), FunctionCallError>, expected: &str) {
    let Err(FunctionCallError::RespondToModel(output)) = result else {
        panic!("expected worktree tool to respond to model with an error");
    };
    assert!(
        output.contains(expected),
        "expected output to contain {expected:?}, got {output:?}"
    );
}

fn response_items_text(items: &[codex_protocol::models::ResponseItem]) -> String {
    let mut text = String::new();
    for item in items {
        let codex_protocol::models::ResponseItem::Message { content, .. } = item else {
            continue;
        };
        for content in content {
            match content {
                codex_protocol::models::ContentItem::InputText { text: item_text }
                | codex_protocol::models::ContentItem::OutputText { text: item_text } => {
                    text.push_str(item_text);
                    text.push('\n');
                }
                codex_protocol::models::ContentItem::InputImage { .. } => {}
                codex_protocol::models::ContentItem::InputAudio { .. } => {}
            }
        }
    }
    text
}
