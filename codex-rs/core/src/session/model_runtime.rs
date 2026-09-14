//! Captured transport and catalog for a turn; pending defaults cannot mutate it.
use super::session::Session;
use super::session::SessionConfiguration;
use crate::client::ModelClient;
use codex_model_provider::SharedModelProvider;
use codex_models_manager::manager::SharedModelsManager;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;
use codex_protocol::protocol::ModelOutputSource;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct ModelRuntime {
    pub(crate) provider: SharedModelProvider,
    pub(crate) client: ModelClient,
    pub(crate) models: SharedModelsManager,
    // Construction also serves previews, which must not fail session startup.
    // Execution checks this result before admitting any model-owned output.
    pub(crate) source: std::result::Result<ModelOutputSource, String>,
}

impl std::fmt::Debug for ModelRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRuntime").finish_non_exhaustive()
    }
}

impl ModelRuntime {
    pub(crate) fn source_for(&self, model: &str) -> Result<ModelOutputSource> {
        let mut source = self.source.clone().map_err(CodexErr::InvalidRequest)?;
        source.model = model.to_owned();
        Ok(source)
    }
}

impl Session {
    pub(super) async fn prepare_model_runtime(
        &self,
        configuration: &SessionConfiguration,
    ) -> Arc<ModelRuntime> {
        let config = &configuration.original_config_do_not_use;
        let provider = if self.services.provider_runtime.load().is_none()
            && self.services.model_client.provider().info() == configuration.provider.info()
        {
            self.services.model_client.provider().clone()
        } else {
            configuration.provider.clone()
        };
        let source = codex_model_provider::model_provider_identity(
            provider.as_ref(),
            &config.model_provider_id,
        )
        .await
        .map(|identity| ModelOutputSource {
            provider_id: config.model_provider_id.clone(),
            model: String::new(),
            identity,
        })
        .map_err(|error| error.to_string());
        if let Some(runtime) = &self.state.lock().await.active_model_runtime
            && source.is_ok()
            && runtime.source == source
            && runtime.provider.info() == provider.info()
        {
            if codex_model_provider_info::responses_preset_for_info(provider.info()).is_some()
                || config.model_catalog.is_some()
            {
                let models = provider.models_manager(
                    config.codex_home.to_path_buf(),
                    config.model_catalog.clone(),
                );
                if models.try_list_models().ok() != runtime.models.try_list_models().ok() {
                    return Arc::new(ModelRuntime {
                        provider,
                        client: runtime.client.clone(),
                        models,
                        source,
                    });
                }
            }
            return runtime.clone();
        }
        let (client, models) = if self.services.provider_runtime.load().is_none()
            && self.services.model_client.provider().info() == provider.info()
        {
            (
                self.services.model_client.clone(),
                self.services.models_manager.clone(),
            )
        } else {
            (
                self.services.model_client(),
                provider.models_manager(
                    config.codex_home.to_path_buf(),
                    config.model_catalog.clone(),
                ),
            )
        };
        Arc::new(ModelRuntime {
            provider,
            client: client.capture_auth_owner(),
            models,
            source,
        })
    }
}
