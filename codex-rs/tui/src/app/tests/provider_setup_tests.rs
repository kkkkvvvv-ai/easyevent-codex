use super::*;
use crate::app::providers::ProviderSetupEvent;
use codex_app_server_protocol::ModelProviderConfigureParams;
use codex_app_server_protocol::ModelProviderInfo;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn provider_site_and_model_confirmation_popups() -> Result<()> {
    let (mut app, _rx, _op_rx) = make_test_app_with_channels().await;
    let mut server = start_config_write_test_app_server(&app).await?;
    let provider = ModelProviderInfo {
        id: "glm-cn".into(),
        family: "GLM".into(),
        name: "GLM (China)".into(),
        base_url: "https://open.bigmodel.cn/api/v1".into(),
        key_instructions: "Use a GLM Coding Plan key.".into(),
        configured: false,
        conflict: false,
        recommended_models: vec!["glm-5.3".into(), "glm-5-turbo".into()],
        supports_model_discovery: false,
    };
    let mut intl = provider.clone();
    intl.id = "glm-intl".into();
    intl.name = "GLM (International / Z.AI)".into();
    app.handle_provider_setup(
        &mut server,
        ProviderSetupEvent::Sites(vec![provider.clone(), intl]),
    );
    insta::assert_snapshot!(
        "provider_site_picker",
        render_bottom_popup(&app.chat_widget, /*width*/ 84)
    );
    app.handle_provider_setup(
        &mut server,
        ProviderSetupEvent::Prepare {
            provider,
            params: ModelProviderConfigureParams {
                provider_id: "glm-cn".into(),
                api_key: "secret-key".into(),
                model: None,
            },
        },
    );
    let rendered = render_bottom_popup(&app.chat_widget, /*width*/ 84);
    assert!(!rendered.contains("secret-key"));
    insta::assert_snapshot!("provider_model_confirmation", rendered);
    Ok(())
}

#[tokio::test]
async fn stale_provider_ack_and_catalog_do_not_replace_newer_selection() -> Result<()> {
    let (mut app, mut events, _ops) = make_test_app_with_channels().await;
    let mut server = start_config_write_test_app_server(&app).await?;
    let origin = app.chat_widget.thread_id();
    let stale = uuid::Uuid::new_v4();
    let latest = uuid::Uuid::new_v4();
    app.provider_selection.latest.insert(origin, latest);
    let provider = app.config.model_provider_id.clone();
    let model = app.config.model.clone();
    let catalog = app.model_catalog.models.clone();
    app.handle_provider_setup(
        &mut server,
        ProviderSetupEvent::Selected(
            origin,
            stale,
            "obsolete".into(),
            "old-model".into(),
            Ok(codex_app_server_protocol::WriteStatus::Ok),
        ),
    );
    app.handle_provider_setup(
        &mut server,
        ProviderSetupEvent::CatalogReloaded(origin, stale, provider.clone(), Ok(Vec::new())),
    );
    assert_eq!(
        (
            app.config.model_provider_id.clone(),
            app.config.model.clone(),
            app.model_catalog.models.clone()
        ),
        (provider, model, catalog)
    );
    assert!(
        !std::iter::from_fn(|| events.try_recv().ok())
            .any(|event| matches!(event, AppEvent::SettingsSelectionSettled))
    );
    Ok(())
}

#[tokio::test]
async fn default_model_write_waits_for_the_confirmed_session_selection() -> Result<()> {
    let (mut app, _events, _ops) = make_test_app_with_channels().await;
    let mut server = start_config_write_test_app_server(&app).await?;
    let mut tui = crate::tui::test_support::make_test_tui()?;
    let thread_id = ThreadId::new();
    app.active_thread_id = Some(thread_id);
    app.pending_model_selections.insert(
        thread_id,
        codex_app_server_protocol::ModelSelection {
            model_provider: "openai".into(),
            model: "gpt-5.5".into(),
        },
    );
    let before = std::fs::read(app.local_settings.user_config_path.as_path()).ok();
    app.handle_event(
        &mut tui,
        &mut server,
        AppEvent::PersistModelSelection {
            model: "gpt-5.5".into(),
            effort: None,
        },
    )
    .await?;
    app.handle_event(
        &mut tui,
        &mut server,
        AppEvent::PersistModelSelection {
            model: "stale-model".into(),
            effort: None,
        },
    )
    .await?;
    assert_eq!(
        std::fs::read(app.local_settings.user_config_path.as_path()).ok(),
        before
    );
    assert_eq!(
        app.pending_model_default_writes.get(&thread_id),
        Some(&("gpt-5.5".into(), None))
    );
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn stale_provider_menu_catalog_does_not_reopen_a_popup() -> Result<()> {
    let (mut app, _events, _ops) = make_test_app_with_channels().await;
    let mut server = start_config_write_test_app_server(&app).await?;
    let origin = app.chat_widget.thread_id();
    let latest = uuid::Uuid::new_v4();
    let provider = ModelProviderInfo {
        id: "deepseek".into(),
        family: "DeepSeek".into(),
        name: "DeepSeek".into(),
        base_url: String::new(),
        key_instructions: String::new(),
        configured: true,
        conflict: false,
        recommended_models: Vec::new(),
        supports_model_discovery: true,
    };
    app.provider_selection.catalog_request = Some((origin, provider.id.clone(), latest));
    let before = render_bottom_popup(&app.chat_widget, /*width*/ 84);
    app.handle_provider_setup(
        &mut server,
        ProviderSetupEvent::ModelsLoaded(
            origin,
            uuid::Uuid::new_v4(),
            provider.clone(),
            Ok(Vec::new()),
        ),
    );
    assert_eq!(render_bottom_popup(&app.chat_widget, /*width*/ 84), before);
    app.handle_provider_setup(
        &mut server,
        ProviderSetupEvent::ModelsLoaded(origin, latest, provider, Ok(Vec::new())),
    );
    assert!(render_bottom_popup(&app.chat_widget, /*width*/ 84).contains("DeepSeek Models"));
    assert!(app.provider_selection.catalog_request.is_none());
    server.shutdown().await?;
    Ok(())
}
