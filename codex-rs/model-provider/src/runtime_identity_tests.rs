use super::*;
use crate::create_model_provider;
use codex_model_provider_info::ModelProviderInfo;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn history_identity_tracks_routing_credentials_but_not_retry_policy() {
    let info = ModelProviderInfo {
        name: "fixture".into(),
        base_url: Some("https://one.example/v1".into()),
        experimental_bearer_token: Some("key-a".into()),
        ..Default::default()
    };
    let provider = create_model_provider(info.clone(), /*auth_manager*/ None);
    let first = model_provider_identity(provider.as_ref(), "a")
        .await
        .unwrap();
    let mut retried = info.clone();
    retried.stream_max_retries = Some(3);
    assert_eq!(
        model_provider_identity(
            create_model_provider(retried, /*auth_manager*/ None).as_ref(),
            "a"
        )
        .await
        .unwrap(),
        first
    );
    for changed in [
        ModelProviderInfo {
            base_url: Some("https://two.example/v1".into()),
            ..info.clone()
        },
        ModelProviderInfo {
            experimental_bearer_token: Some("key-b".into()),
            ..info
        },
    ] {
        assert_ne!(
            model_provider_identity(
                create_model_provider(changed, /*auth_manager*/ None).as_ref(),
                "a"
            )
            .await
            .unwrap(),
            first
        );
    }
    assert_ne!(
        model_provider_identity(provider.as_ref(), "b")
            .await
            .unwrap(),
        first
    );
}

#[tokio::test]
async fn history_identity_survives_token_refresh_but_separates_users_and_workspaces() {
    use base64::Engine;
    let mut identities = Vec::new();
    for (user, workspace, token) in [
        ("user-a", "workspace-a", "token-1"),
        ("user-a", "workspace-a", "token-2"),
        ("user-b", "workspace-a", "token-3"),
        ("user-a", "workspace-b", "token-4"),
    ] {
        let claims = serde_json::json!({"jti": token, "https://api.openai.com/auth": {"chatgpt_user_id": user}});
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        let auth = CodexAuth::from_external_chatgpt_tokens(
            &format!("header.{payload}.signature"),
            workspace,
            /*chatgpt_plan_type*/ None,
        )
        .unwrap();
        let provider = create_model_provider(
            ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            Some(codex_login::AuthManager::from_auth_for_testing(auth)),
        );
        identities.push(
            model_provider_identity(provider.as_ref(), "openai")
                .await
                .unwrap(),
        );
    }
    assert_eq!(identities[0], identities[1]);
    assert_ne!(identities[0], identities[2]);
    assert_ne!(identities[0], identities[3]);
}
