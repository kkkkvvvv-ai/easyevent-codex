//! Restore the last producer separately from startup or queued model defaults.
use super::session::Session;
use super::session::SessionSettingsUpdate;
use super::step_settings::ResolvedStepSettings;
use super::step_settings::StepSettingsUpdate;
use codex_history::ResponseItemEnvelope;
use codex_protocol::protocol::TurnContextItem;
use std::sync::Arc;

impl Session {
    pub(super) async fn restore_model_runtime(
        &self,
        history: &[ResponseItemEnvelope],
        context: Option<&TurnContextItem>,
        rollout: &[codex_history::RolloutItem],
    ) {
        let active = crate::thread_model_selection::latest_persisted_owned_thread_settings(
            rollout,
            self.thread_id(),
        )
        .and_then(|settings| settings.active_model.as_ref());
        let producers = || {
            history
                .iter()
                .rev()
                .filter_map(|item| item.metadata.as_ref()?.model_source.clone())
        };
        let source = producers()
            .find(|source| {
                active.is_none_or(|active| {
                    active.model_provider == source.provider_id && active.model == source.model
                })
            })
            .or_else(|| {
                context
                    .and_then(|item| item.model_source.clone())
                    .filter(|source| {
                        active.is_some_and(|active| {
                            active.model_provider == source.provider_id
                                && active.model == source.model
                        })
                    })
            })
            // A rollback can remove every output of the previously active selection.
            .or_else(|| producers().next());
        let Some(source) = source else {
            let mut state = self.state.lock().await;
            state.active_model_runtime = None;
            state.active_model_settings = None;
            state.active_model_source = None;
            return;
        };
        // Keep provenance even if the old credentials are no longer available.
        {
            let mut state = self.state.lock().await;
            state.active_model_runtime = None;
            state.active_model_settings = None;
            state.active_model_source = Some(source.clone());
        }
        let configuration = self.state.lock().await.session_configuration.clone();
        let updates = SessionSettingsUpdate {
            model_provider: Some(source.provider_id.clone()),
            step_settings: StepSettingsUpdate {
                model: Some(source.model.clone()),
                effort: context.map(|item| item.effort.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let Ok(configuration) = self.apply_session_settings(&configuration, &updates) else {
            return;
        };
        let runtime = self.prepare_model_runtime(&configuration).await;
        if runtime.source_for(&source.model).ok().as_ref() != Some(&source) {
            return;
        }
        let model = configuration
            .step_settings
            .resolve_model_info(
                runtime.models.as_ref(),
                &configuration.model_info_overrides,
                self.features.enabled(codex_features::Feature::Personality),
            )
            .await;
        let settings = Arc::new(ResolvedStepSettings::new(
            configuration.step_settings,
            Arc::new(model),
            self.features.enabled(codex_features::Feature::FastMode),
        ));
        let mut state = self.state.lock().await;
        state.active_model_runtime = Some(runtime);
        state.active_model_settings = Some(settings);
    }
}

#[cfg(test)]
#[path = "provider_restore_tests.rs"]
mod tests;
