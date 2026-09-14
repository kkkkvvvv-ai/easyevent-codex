use super::*;
use codex_model_provider_info::responses_provider_preset;
use pretty_assertions::assert_eq;

#[test]
fn provider_keys_survive_openai_logout_and_are_isolated_by_site() {
    let home = tempfile::tempdir().unwrap();
    let store = ProviderCredentialStore::new(
        home.path().to_path_buf(),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::Direct,
    );
    let cn = responses_provider_preset("glm-cn").unwrap();
    let intl = responses_provider_preset("glm-intl").unwrap();
    store.write(cn, " cn-key ").unwrap();
    store.write(intl, "intl-key").unwrap();
    login_with_api_key(
        home.path(),
        "openai-key",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::Direct,
    )
    .unwrap();
    logout(
        home.path(),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::Direct,
    )
    .unwrap();
    assert_eq!(
        (store.read(cn).unwrap(), store.read(intl).unwrap()),
        (Some("cn-key".into()), Some("intl-key".into()))
    );
    assert!(store.write(cn, "\n").is_err());
    assert_eq!(store.read(cn).unwrap(), Some("cn-key".into()));
    assert!(store.delete(cn).unwrap());
    assert_eq!(
        (store.read(cn).unwrap(), store.read(intl).unwrap()),
        (None, Some("intl-key".into()))
    );
}

#[test]
fn ephemeral_credentials_do_not_touch_disk_or_follow_endpoint_changes() {
    let home = tempfile::tempdir().unwrap();
    let store = ProviderCredentialStore::new(
        home.path().to_path_buf(),
        AuthCredentialsStoreMode::Ephemeral,
        AuthKeyringBackendKind::Direct,
    );
    let preset = responses_provider_preset("deepseek").unwrap();
    store.write(preset, "ephemeral-key").unwrap();
    let changed = ResponsesProviderPreset {
        base_url: "https://different.example/v1",
        ..*preset
    };
    assert_eq!(store.read(&changed).unwrap(), None);
    assert_eq!(store.read(preset).unwrap(), Some("ephemeral-key".into()));
    assert!(!home.path().join("provider-auth").exists());
}
