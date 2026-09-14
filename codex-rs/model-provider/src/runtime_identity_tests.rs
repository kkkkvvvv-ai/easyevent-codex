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
