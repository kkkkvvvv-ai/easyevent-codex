use super::*;
use crate::app::providers::ProviderSetupEvent;
use codex_app_server_protocol::ModelProviderConfigureParams;
use codex_app_server_protocol::ModelProviderInfo;

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
