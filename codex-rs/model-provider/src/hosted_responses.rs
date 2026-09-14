//! Native hosted Responses providers with endpoint-scoped credentials.

use std::path::PathBuf;
use std::sync::Arc;

use codex_api::SharedAuthProvider;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_login::auth::ProviderCredentialStore;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::ResponsesProviderPreset;
use codex_models_manager::hosted_responses_model;
use codex_models_manager::manager::SharedModelsManager;
use codex_models_manager::manager::StaticModelsManager;
use codex_protocol::error::CodexErr;
use codex_protocol::openai_models::ModelsResponse;

use crate::BearerAuthProvider;
use crate::ModelProvider;
use crate::ModelProviderFuture;
use crate::ProviderAccountResult;
use crate::ProviderAccountState;
use crate::ProviderCapabilities;
use crate::RemoteCompactionSupport;

#[derive(Debug)]
pub(crate) struct HostedResponsesProvider {
    pub(crate) info: ModelProviderInfo,
    pub(crate) preset: &'static ResponsesProviderPreset,
    pub(crate) credentials: Option<ProviderCredentialStore>,
}

pub fn recommended_responses_models(preset: &ResponsesProviderPreset) -> ModelsResponse {
    ModelsResponse {
        models: preset
            .models
            .iter()
            .enumerate()
            .map(|(priority, slug)| {
                let mut model = hosted_responses_model(slug);
                model.priority = priority as i32;
                model
            })
            .collect(),
    }
}

impl ModelProvider for HostedResponsesProvider {
    fn info(&self) -> &ModelProviderInfo {
        &self.info
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            namespace_tools: false,
            image_generation: false,
            web_search: false,
            external_web_access: false,
            remote_compaction: RemoteCompactionSupport::Unsupported,
        }
    }

    fn approval_review_preferred_model(&self) -> &'static str {
        self.preset.models[0]
    }
    fn memory_extraction_preferred_model(&self) -> &'static str {
        self.preset.models[0]
    }
    fn memory_consolidation_preferred_model(&self) -> &'static str {
        self.preset.models[0]
    }
    fn auth_manager(&self) -> Option<Arc<AuthManager>> {
        None
    }
    fn auth(&self) -> ModelProviderFuture<'_, Option<CodexAuth>> {
        Box::pin(async { None })
    }

    fn account_state(&self) -> ProviderAccountResult {
        Ok(ProviderAccountState {
            account: None,
            requires_openai_auth: false,
        })
    }

    fn api_auth(
        &self,
    ) -> ModelProviderFuture<'_, codex_protocol::error::Result<SharedAuthProvider>> {
        Box::pin(async {
            let key = self
                .credentials
                .as_ref()
                .map(|store| store.read(self.preset))
                .transpose()?
                .flatten()
                .ok_or_else(|| {
                    CodexErr::InvalidRequest(format!(
                        "{} API key is missing. Use /model to connect this provider.",
                        self.preset.name
                    ))
                })?;
            Ok(Arc::new(BearerAuthProvider::new(key)) as SharedAuthProvider)
        })
    }

    fn models_manager(
        &self,
        _codex_home: PathBuf,
        catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager {
        self.models_manager_without_cache(catalog)
    }

    fn models_manager_without_cache(&self, catalog: Option<ModelsResponse>) -> SharedModelsManager {
        Arc::new(StaticModelsManager::new(
            None,
            catalog.unwrap_or_else(|| {
                self.credentials
                    .as_ref()
                    .and_then(|store| {
                        store
                            .read(self.preset)
                            .ok()
                            .flatten()
                            .map(|key| crate::cached_responses_models(store, self.preset, &key))
                    })
                    .unwrap_or_else(|| recommended_responses_models(self.preset))
            }),
        ))
    }
}
