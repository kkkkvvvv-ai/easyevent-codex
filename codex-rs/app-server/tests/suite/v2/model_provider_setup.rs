use anyhow::Result;
use app_test_support::TestAppServer;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ModelListParams;
use codex_app_server_protocol::ModelListResponse;
use codex_app_server_protocol::ModelProviderConfigureParams;
use codex_app_server_protocol::ModelProviderCredentialDeleteParams;
use codex_app_server_protocol::ModelProviderCredentialDeleteResponse;
use codex_app_server_protocol::ModelProviderListParams;
use codex_app_server_protocol::ModelProviderListResponse;
use codex_app_server_protocol::ThreadHistoryMode;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadSettingsUpdateParams;
use codex_app_server_protocol::ThreadSettingsUpdateResponse;
use codex_app_server_protocol::ThreadSettingsUpdatedNotification;
use codex_app_server_protocol::ThreadStartParams;
use codex_login::AuthCredentialsStoreMode;
use codex_login::AuthKeyringBackendKind;
use codex_login::auth::ProviderCredentialStore;
use codex_model_provider_info::responses_provider_preset;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use test_case::test_case;

#[tokio::test]
async fn provider_catalog_paginates_without_exposing_credentials() -> Result<()> {
    let home = tempfile::tempdir()?;
    let store = ProviderCredentialStore::new(
        home.path().to_path_buf(),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::Direct,
    );
    store.write(
        responses_provider_preset("deepseek").unwrap(),
        "secret-provider-key",
    )?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let first: ModelProviderListResponse = server
        .request(|request_id| ClientRequest::ModelProviderList {
            request_id,
            params: ModelProviderListParams {
                cursor: None,
                limit: Some(2),
            },
        })
        .await?;
    assert_eq!(first.data.len(), 2);
    assert!(first.data[0].configured);
    assert!(!serde_json::to_string(&first)?.contains("secret-provider-key"));
    let second: ModelProviderListResponse = server
        .request(|request_id| ClientRequest::ModelProviderList {
            request_id,
            params: ModelProviderListParams {
                cursor: first.next_cursor,
                limit: Some(20),
            },
        })
        .await?;
    assert_eq!(second.data.len(), 7);
    assert_eq!(second.next_cursor, None);
    let models: ModelListResponse = server
        .request(|request_id| ClientRequest::ModelList {
            request_id,
            params: ModelListParams {
                provider_id: Some("kimi".into()),
                ..Default::default()
            },
        })
        .await?;
    assert_eq!(
        models
            .data
            .into_iter()
            .map(|model| model.model)
            .collect::<Vec<_>>(),
        vec!["kimi-for-coding", "k3-256k", "k3"]
    );
    Ok(())
}

#[test_case(ThreadHistoryMode::Legacy; "legacy")]
#[test_case(ThreadHistoryMode::Paginated; "paginated")]
#[tokio::test]
async fn provider_switch_api_persists_selection_for_cold_resume(
    mode: ThreadHistoryMode,
) -> Result<()> {
    let first = responses::start_mock_server().await;
    let second = responses::start_mock_server().await;
    let home = tempfile::tempdir()?;
    app_test_support::MockResponsesConfig::new(&first.uri()).with_root_config(&format!(
        "[model_providers.provider-b]\nname = \"Provider B\"\nbase_url = \"{}/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"key-b\"", second.uri()
    )).write(home.path())?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let started = server
        .start_thread(ThreadStartParams {
            history_mode: Some(mode),
            ..Default::default()
        })
        .await?;
    let thread_id = started.thread.id;
    let _: ThreadSettingsUpdateResponse = server
        .request(|request_id| ClientRequest::ThreadSettingsUpdate {
            request_id,
            params: ThreadSettingsUpdateParams {
                thread_id: thread_id.clone(),
                model_provider: Some("provider-b".into()),
                model: Some("mock-model-b".into()),
                ..Default::default()
            },
        })
        .await?;
    let notification: ThreadSettingsUpdatedNotification =
        server.read_notification("thread/settings/updated").await?;
    assert_eq!(
        (
            notification.thread_id,
            notification.thread_settings.model_provider
        ),
        (thread_id.clone(), "provider-b".into())
    );
    server.shutdown_gracefully().await?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let resumed: ThreadResumeResponse = server
        .request(|request_id| ClientRequest::ThreadResume {
            request_id,
            params: ThreadResumeParams {
                thread_id: thread_id.clone(),
                ..Default::default()
            },
        })
        .await?;
    assert_eq!(
        (resumed.thread.id, resumed.model_provider, resumed.model),
        (thread_id, "provider-b".into(), "mock-model-b".into())
    );
    server.shutdown_gracefully().await?;
    Ok(())
}

#[tokio::test]
async fn failed_setup_preserves_key_and_delete_only_removes_selected_site() -> Result<()> {
    let home = tempfile::tempdir()?;
    let store = ProviderCredentialStore::new(
        home.path().to_path_buf(),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::Direct,
    );
    let cn = responses_provider_preset("glm-cn").unwrap();
    let intl = responses_provider_preset("glm-intl").unwrap();
    store.write(cn, "old-key")?;
    store.write(intl, "other-site-key")?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let request_id = server
        .send_request(
            "modelProvider/configure",
            Some(serde_json::to_value(ModelProviderConfigureParams {
                provider_id: "glm-cn".into(),
                api_key: " ".into(),
                model: None,
            })?),
        )
        .await?;
    let failed = server
        .read_stream_until_error_message(codex_app_server_protocol::RequestId::Integer(request_id))
        .await?;
    assert_eq!(
        failed.error.message,
        "API key must be nonempty and contain no control characters"
    );
    assert_eq!(store.read(cn)?, Some("old-key".into()));
    let deleted: ModelProviderCredentialDeleteResponse = server
        .request(|request_id| ClientRequest::ModelProviderCredentialDelete {
            request_id,
            params: ModelProviderCredentialDeleteParams {
                provider_id: "glm-cn".into(),
            },
        })
        .await?;
    assert!(deleted.deleted);
    assert_eq!(
        (store.read(cn)?, store.read(intl)?),
        (None, Some("other-site-key".into()))
    );
    Ok(())
}
