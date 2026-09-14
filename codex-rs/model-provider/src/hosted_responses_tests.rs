use super::*;
use codex_login::AuthCredentialsStoreMode;
use codex_login::AuthKeyringBackendKind;
use codex_model_provider_info::responses_provider_preset;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn hosted_credentials_keep_an_existing_thread_on_its_original_identity() {
    let home = tempfile::tempdir().unwrap();
    let store = ProviderCredentialStore::new(
        home.path().to_path_buf(),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::Direct,
    );
    let preset = responses_provider_preset("deepseek").unwrap();
    store.write(preset, "original-key").unwrap();
    let provider = HostedResponsesProvider {
        info: preset.provider_info(),
        preset,
        credentials: Some(store.clone()),
        api_key: Some("original-key".into()),
    };
    let auth = provider.api_auth().await.unwrap();
    let mut headers = http::HeaderMap::new();
    auth.add_auth_headers(&mut headers);
    assert_eq!(headers.get("authorization").unwrap(), "Bearer original-key");
    assert!(!format!("{provider:?}").contains("original-key"));
    store.write(preset, "replacement-key").unwrap();
    let unchanged = provider.api_auth().await.unwrap();
    let mut headers = http::HeaderMap::new();
    unchanged.add_auth_headers(&mut headers);
    assert_eq!(headers.get("authorization").unwrap(), "Bearer original-key");
    let replacement = HostedResponsesProvider {
        info: preset.provider_info(),
        preset,
        credentials: Some(store.clone()),
        api_key: Some("replacement-key".into()),
    };
    let auth = replacement.api_auth().await.unwrap();
    let mut headers = http::HeaderMap::new();
    auth.add_auth_headers(&mut headers);
    assert_eq!(
        headers.get("authorization").unwrap(),
        "Bearer replacement-key"
    );
    store.delete(preset).unwrap();
    assert!(replacement.api_auth().await.is_err());
    assert!(provider.api_auth().await.is_err());
}
