use super::*;
use pretty_assertions::assert_eq;

fn auth_for_user(user_id: &str) -> AuthDotJson {
    let mut tokens = build_tokens(INITIAL_ACCESS_TOKEN, INITIAL_REFRESH_TOKEN);
    tokens.id_token = IdTokenInfo {
        raw_jwt: jwt_with_payload(json!({
            "sub": user_id,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "account-id",
                "chatgpt_user_id": user_id,
                "user_id": user_id,
            }
        })),
        chatgpt_account_id: Some("account-id".to_string()),
        chatgpt_user_id: Some(user_id.to_string()),
        ..Default::default()
    };
    AuthDotJson {
        auth_mode: Some(AuthMode::Chatgpt),
        openai_api_key: None,
        tokens: Some(tokens),
        last_refresh: Some(Utc::now()),
        agent_identity: None,
        personal_access_token: None,
        bedrock_api_key: None,
        bedrock_access_keys: None,
    }
}

#[serial_test::serial(auth_env)]
#[tokio::test]
async fn unauthorized_recovery_fences_user_change_within_workspace() -> Result<()> {
    check_reload_user_change(RecoveryCache::Original).await
}

#[serial_test::serial(auth_env)]
#[tokio::test]
async fn unauthorized_recovery_fences_user_change_already_loaded_by_another_caller() -> Result<()> {
    check_reload_user_change(RecoveryCache::Reloaded).await
}

enum RecoveryCache {
    Original,
    Reloaded,
}

async fn check_reload_user_change(cache: RecoveryCache) -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let ctx = RefreshTokenTestContext::new(&server).await?;
    ctx.write_auth(&auth_for_user("initial-user")).await?;
    let replacement = auth_for_user("replacement-user");
    save_auth(
        ctx.codex_home.path(),
        &replacement,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;
    let mut recovery = ctx.auth_manager.unauthorized_recovery();
    if matches!(cache, RecoveryCache::Reloaded) {
        ctx.auth_manager.reload().await;
    }

    let error = recovery
        .next()
        .await
        .expect_err("reloading another user must stop the active request");

    assert!(error.is_account_changed());
    assert!(!recovery.has_next());
    assert_eq!(ctx.load_auth()?, replacement);
    assert_eq!(
        ctx.auth_manager
            .auth_cached()
            .context("replacement auth should be cached for future requests")?
            .get_token_data()?,
        replacement.tokens.context("replacement tokens")?
    );
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        0
    );
    Ok(())
}

#[serial_test::serial(auth_env)]
#[tokio::test]
async fn proactive_refresh_fences_user_change_within_workspace() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let replacement = auth_for_user("replacement-user");
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id_token": replacement.tokens.as_ref().context("replacement tokens")?.id_token.raw_jwt,
            "access_token": "replacement-access-token",
            "refresh_token": "replacement-refresh-token",
        })))
        .expect(1)
        .mount(&server)
        .await;
    let ctx = RefreshTokenTestContext::new(&server).await?;
    let initial = auth_for_user("initial-user");
    ctx.write_auth(&initial).await?;

    let error = ctx
        .auth_manager
        .refresh_token_from_authority()
        .await
        .expect_err("refreshing into another user must be rejected before persistence");

    assert!(error.is_account_changed());
    assert_eq!(ctx.load_auth()?, initial);
    assert_eq!(
        ctx.auth_manager
            .auth_cached()
            .context("original auth should remain cached")?
            .get_token_data()?,
        initial.tokens.context("initial tokens")?
    );
    server.verify().await;
    Ok(())
}
