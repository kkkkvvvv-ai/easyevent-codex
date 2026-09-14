//! Captured transport and catalog for a turn; pending defaults cannot mutate it.
use super::session::Session;
use super::session::SessionConfiguration;
use super::step_context::StepContext;
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
                self.services.model_client.with_provider(provider.clone()),
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

    pub(super) async fn activate_model_runtime(
        &self,
        step: &StepContext,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        let turn = &step.turn;
        let source = turn
            .model_runtime
            .source_for(&step.settings.model_info.slug)?;
        let _guard = super::thread_settings::acquire_persistence_lock(self).await;
        if cancellation.is_cancelled() {
            return Err(CodexErr::TurnAborted);
        }
        let previously_active = self.state.lock().await.active_model_source.is_some();
        let mut snapshot = self.thread_settings_snapshot().await;
        snapshot.active_model = Some(codex_protocol::protocol::ProviderModelSelection {
            model_provider: source.provider_id.clone(),
            model: source.model.clone(),
        });
        if previously_active && let Some(live) = self.live_thread() {
            live.append_items(&[codex_history::RolloutItem::EventMsg(
                codex_protocol::protocol::EventMsg::ThreadSettingsApplied(
                    codex_protocol::protocol::ThreadSettingsAppliedEvent {
                        thread_id: Some(self.thread_id()),
                        thread_settings: snapshot.clone(),
                    },
                ),
            )])
            .await
            .map_err(|error| CodexErr::InvalidRequest(error.to_string()))?;
            live.flush()
                .await
                .map_err(|error| CodexErr::InvalidRequest(error.to_string()))?;
        }
        {
            let mut state = self.state.lock().await;
            state.active_model_runtime = Some(turn.model_runtime.clone());
            state.active_model_settings = Some(step.settings.clone());
            state.active_model_source = Some(source);
        }
        self.services
            .provider_runtime
            .store(Some(Arc::new(crate::state::ProviderRuntime {
                model_client: turn.model_runtime.client.clone(),
                models_manager: turn.model_runtime.models.clone(),
            })));
        if previously_active {
            super::thread_settings::emit_applied(self, turn.sub_id.clone(), snapshot).await;
        }
        Ok(())
    }
}
