//! Hosted provider catalog, verified credential setup and model discovery.

use super::catalog_processor::CatalogRequestProcessor;
use crate::error_code::internal_error;
use crate::error_code::invalid_request;
use codex_app_server_protocol::ClientResponsePayload;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::ModelCatalogMode;
use codex_app_server_protocol::ModelListParams;
use codex_app_server_protocol::ModelListResponse;
use codex_app_server_protocol::ModelProviderConfigureParams;
use codex_app_server_protocol::ModelProviderConfigureResponse;
use codex_app_server_protocol::ModelProviderCredentialDeleteParams;
use codex_app_server_protocol::ModelProviderCredentialDeleteResponse;
use codex_app_server_protocol::ModelProviderInfo;
use codex_app_server_protocol::ModelProviderListParams;
use codex_app_server_protocol::ModelProviderListResponse;
use codex_core::config::Config;
use codex_login::auth::ProviderCredentialStore;
use codex_model_provider::cache_responses_models;
use codex_model_provider::cached_responses_models;
use codex_model_provider::discover_responses_models;
use codex_model_provider::recommended_responses_models;
use codex_model_provider::validate_responses_key;
use codex_model_provider_info::RESPONSES_PROVIDER_PRESETS;
use codex_model_provider_info::ResponsesProviderPreset;
use codex_model_provider_info::responses_provider_preset;
use codex_protocol::config_types::ForcedLoginMethod;

impl CatalogRequestProcessor {
    pub(crate) async fn model_provider_list(
        &self,
        params: ModelProviderListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let config = self
            .config_manager
            .load_latest_config(/*fallback_cwd*/ None)
            .await
            .map_err(|_| internal_error("Unable to load provider configuration"))?;
        let store = provider_store(&config);
        let data = RESPONSES_PROVIDER_PRESETS
            .iter()
            .map(|preset| {
                let conflict = config
                    .model_providers
                    .get(preset.id)
                    .is_some_and(|info| info != &preset.provider_info());
                Ok(ModelProviderInfo {
                    id: preset.id.into(),
                    family: preset.family.into(),
                    name: preset.name.into(),
                    base_url: preset.base_url.into(),
                    key_instructions: preset.key_instructions.into(),
                    configured: store
                        .read(preset)
                        .map_err(|_| internal_error("Unable to read provider credentials"))?
                        .is_some(),
                    conflict,
                    recommended_models: preset.models.iter().map(|model| (*model).into()).collect(),
                    supports_model_discovery: preset.discovery,
                })
            })
            .collect::<Result<Vec<_>, JSONRPCErrorError>>()?;
        let (data, next_cursor) = page(data, params.cursor, params.limit)?;
        Ok(Some(ModelProviderListResponse { data, next_cursor }.into()))
    }

    pub(crate) async fn model_provider_configure(
        &self,
        params: ModelProviderConfigureParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let config = self
            .config_manager
            .load_latest_config(/*fallback_cwd*/ None)
            .await
            .map_err(|_| internal_error("Unable to load provider configuration"))?;
        let preset = configured_preset(&config, &params.provider_id)?;
        if !self
            .thread_manager
            .auth_manager()
            .is_login_method_allowed(ForcedLoginMethod::Api)
        {
            return Err(invalid_request(
                "API key login is disabled by managed policy",
            ));
        }
        if let Some(required) = config.config_layer_stack.required_model_provider()
            && required != preset.id
        {
            return Err(invalid_request(
                "This provider is disabled by managed policy",
            ));
        }
        let model = params.model.unwrap_or_else(|| preset.models[0].to_string());
        validate_responses_key(
            config.http_client_factory(),
            preset,
            &params.api_key,
            &model,
        )
        .await
        .map_err(invalid_request)?;
        provider_store(&config)
            .write(preset, &params.api_key)
            .map_err(|_| internal_error("Unable to save provider credentials"))?;
        Ok(Some(
            ModelProviderConfigureResponse {
                provider_id: preset.id.into(),
                model,
                configured: true,
            }
            .into(),
        ))
    }

    pub(crate) async fn model_provider_credential_delete(
        &self,
        params: ModelProviderCredentialDeleteParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let preset = responses_provider_preset(&params.provider_id)
            .ok_or_else(|| invalid_request("Unknown provider"))?;
        let config = self
            .config_manager
            .load_latest_config(/*fallback_cwd*/ None)
            .await
            .map_err(|_| internal_error("Unable to load provider configuration"))?;
        let deleted = provider_store(&config)
            .delete(preset)
            .map_err(|_| internal_error("Unable to delete provider credentials"))?;
        Ok(Some(
            ModelProviderCredentialDeleteResponse { deleted }.into(),
        ))
    }

    pub(super) async fn hosted_model_list(
        &self,
        params: ModelListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let config = self
            .config_manager
            .load_latest_config(/*fallback_cwd*/ None)
            .await
            .map_err(|_| internal_error("Unable to load provider configuration"))?;
        let id = params
            .provider_id
            .as_deref()
            .unwrap_or(&config.model_provider_id);
        if responses_provider_preset(id).is_none() {
            let info = config
                .model_providers
                .get(id)
                .cloned()
                .ok_or_else(|| invalid_request("Unknown provider"))?;
            let provider = codex_model_provider::create_model_provider(
                info,
                Some(self.thread_manager.auth_manager()),
            );
            let manager = provider.models_manager(
                config.codex_home.to_path_buf(),
                config.model_catalog.clone(),
            );
            manager.set_api_key_model_discovery_enabled(
                config
                    .features
                    .enabled(codex_features::Feature::ApiKeyModelDiscovery),
            );
            let models = manager
                .list_models(
                    codex_models_manager::manager::RefreshStrategy::OnlineIfUncached,
                    config.http_client_factory(),
                )
                .await
                .into_iter()
                .filter(|model| params.include_hidden.unwrap_or(false) || model.show_in_picker)
                .map(crate::models::model_from_preset)
                .collect();
            let (data, next_cursor) = page(models, params.cursor, params.limit)?;
            return Ok(Some(ModelListResponse { data, next_cursor }.into()));
        }
        let preset = configured_preset(&config, id)?;
        let store = provider_store(&config);
        let catalog = if params.catalog_mode == Some(ModelCatalogMode::All) {
            let key = store
                .read(preset)
                .map_err(|_| internal_error("Unable to read provider credentials"))?
                .ok_or_else(|| {
                    invalid_request("Connect this provider before loading more models")
                })?;
            if params.refresh.unwrap_or(true) {
                match discover_responses_models(config.http_client_factory(), preset, &key).await {
                    Ok(catalog) => {
                        cache_responses_models(&store, preset, &key, catalog.clone()).map_err(
                            |_| internal_error("Unable to cache provider model metadata"),
                        )?;
                        catalog
                    }
                    Err(message) => {
                        // Explicit refresh errors let clients retain their existing cached list.
                        return Err(invalid_request(message));
                    }
                }
            } else {
                cached_responses_models(&store, preset, &key)
            }
        } else {
            recommended_responses_models(preset)
        };
        let models = catalog
            .models
            .into_iter()
            .map(|model| crate::models::model_from_preset(model.into()))
            .collect();
        let (data, next_cursor) = page(models, params.cursor, params.limit)?;
        Ok(Some(ModelListResponse { data, next_cursor }.into()))
    }
}

pub(super) fn configured_preset(
    config: &Config,
    id: &str,
) -> Result<&'static ResponsesProviderPreset, JSONRPCErrorError> {
    let preset =
        responses_provider_preset(id).ok_or_else(|| invalid_request("Unknown hosted provider"))?;
    if config
        .model_providers
        .get(id)
        .is_some_and(|info| info != &preset.provider_info())
    {
        return Err(invalid_request(
            "This provider ID has a custom configuration. Rename the custom entry before using the built-in setup.",
        ));
    }
    Ok(preset)
}

pub(super) fn provider_store(config: &Config) -> ProviderCredentialStore {
    ProviderCredentialStore::new(
        config.codex_home.to_path_buf(),
        config.cli_auth_credentials_store_mode,
        config.auth_keyring_backend_kind(),
    )
}

fn page<T: Clone>(
    items: Vec<T>,
    cursor: Option<String>,
    limit: Option<u32>,
) -> Result<(Vec<T>, Option<String>), JSONRPCErrorError> {
    let start = cursor
        .as_deref()
        .unwrap_or("0")
        .parse::<usize>()
        .map_err(|_| invalid_request("Invalid catalog cursor"))?;
    if start > items.len() {
        return Err(invalid_request("Catalog cursor is out of range"));
    }
    let end = start
        .saturating_add(limit.unwrap_or(100).clamp(1, 500) as usize)
        .min(items.len());
    Ok((
        items[start..end].to_vec(),
        (end < items.len()).then(|| end.to_string()),
    ))
}
