//! Provider setup and selection use the owning app server, including remote sessions.

use super::App;
use crate::app_event::AppEvent;
use crate::app_server_session::AppServerSession;
use crate::bottom_pane::ProviderKeyView;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::Model;
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
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadSettingsUpdateParams;
use codex_app_server_protocol::ThreadSettingsUpdateResponse;
use codex_app_server_protocol::WriteStatus;
use codex_protocol::ThreadId;
use codex_protocol::openai_models::ReasoningEffort;
use serde_json::json;

#[derive(Default)]
pub(super) struct ProviderSelectionState {
    pub(super) pending: std::collections::HashMap<ThreadId, uuid::Uuid>,
    pub(super) latest: std::collections::HashMap<Option<ThreadId>, uuid::Uuid>,
    pub(super) catalog_request: Option<(Option<ThreadId>, String, uuid::Uuid)>,
    task: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Debug)]
pub(crate) enum ProviderSetupEvent {
    Open,
    Listed(Option<ThreadId>, Result<Vec<ModelProviderInfo>, String>),
    Sites(Vec<ModelProviderInfo>),
    Site(ModelProviderInfo),
    Key(ModelProviderInfo),
    Prepare {
        provider: ModelProviderInfo,
        params: ModelProviderConfigureParams,
    },
    Configure {
        provider: ModelProviderInfo,
        params: ModelProviderConfigureParams,
    },
    Configured(
        Option<ThreadId>,
        ModelProviderInfo,
        Result<ModelProviderConfigureResponse, String>,
    ),
    Models {
        provider: ModelProviderInfo,
        mode: ModelCatalogMode,
    },
    ModelsLoaded(
        Option<ThreadId>,
        uuid::Uuid,
        ModelProviderInfo,
        Result<Vec<Model>, String>,
    ),
    Choose(ModelProviderInfo, Box<Model>),
    Select {
        provider: ModelProviderInfo,
        model: String,
        effort: Option<ReasoningEffort>,
    },
    Selected(
        Option<ThreadId>,
        uuid::Uuid,
        String,
        String,
        Result<WriteStatus, String>,
    ),
    Delete(ModelProviderInfo),
    Deleted(Option<ThreadId>, Result<(), String>),
    CatalogReloaded(
        Option<ThreadId>,
        uuid::Uuid,
        String,
        Result<Vec<Model>, String>,
    ),
    CatalogWarning(Option<ThreadId>, String),
}

fn request_id() -> RequestId {
    RequestId::String(uuid::Uuid::new_v4().to_string())
}

impl From<ProviderSetupEvent> for AppEvent {
    fn from(event: ProviderSetupEvent) -> Self {
        Self::ProviderSetup(Box::new(event))
    }
}

pub(crate) fn provider_selection_item() -> SelectionItem {
    SelectionItem {
        name: "Switch / connect provider".into(),
        description: Some("DeepSeek, GLM, Kimi, HY, MiniMax, OpenRouter".into()),
        actions: vec![Box::new(|tx| {
            tx.send(AppEvent::from(ProviderSetupEvent::Open))
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

impl App {
    pub(super) fn handle_provider_setup(
        &mut self,
        app_server: &mut AppServerSession,
        event: ProviderSetupEvent,
    ) {
        let handle = app_server.request_handle();
        let tx = self.app_event_tx.clone();
        let thread_id = self.chat_widget.thread_id();
        match event {
            ProviderSetupEvent::Open => {
                self.provider_selection.catalog_request = None;
                tokio::spawn(async move {
                    let result = handle
                        .request_typed::<ModelProviderListResponse>(
                            ClientRequest::ModelProviderList {
                                request_id: request_id(),
                                params: ModelProviderListParams::default(),
                            },
                        )
                        .await
                        .map(|response| response.data)
                        .map_err(|error| error.to_string());
                    tx.send(AppEvent::from(ProviderSetupEvent::Listed(
                        thread_id, result,
                    )));
                });
            }
            ProviderSetupEvent::Listed(origin, result) if origin == thread_id => match result {
                Ok(providers) => {
                    let mut families: Vec<(String, Vec<ModelProviderInfo>)> = Vec::new();
                    for provider in providers {
                        if let Some((_, sites)) = families
                            .iter_mut()
                            .find(|(family, _)| family == &provider.family)
                        {
                            sites.push(provider);
                        } else {
                            families.push((provider.family.clone(), vec![provider]));
                        }
                    }
                    let mut items: Vec<SelectionItem> = families
                        .into_iter()
                        .map(|(family, sites)| SelectionItem {
                            name: family,
                            actions: vec![Box::new(move |tx| {
                                tx.send(AppEvent::from(ProviderSetupEvent::Sites(sites.clone())))
                            })],
                            dismiss_on_select: true,
                            ..Default::default()
                        })
                        .collect();
                    let openai = ModelProviderInfo {
                        id: "openai".into(),
                        family: "OpenAI".into(),
                        name: "OpenAI".into(),
                        base_url: String::new(),
                        key_instructions: String::new(),
                        configured: true,
                        conflict: false,
                        recommended_models: Vec::new(),
                        supports_model_discovery: false,
                    };
                    items.insert(0, site_item(openai));
                    self.chat_widget.show_selection_view(SelectionViewParams {
                        title: Some("Select Provider".into()),
                        items,
                        ..Default::default()
                    });
                }
                Err(error) => self.chat_widget.add_error_message(error),
            },
            ProviderSetupEvent::Sites(mut sites) => {
                if sites.len() == 1 {
                    self.app_event_tx
                        .send(AppEvent::from(ProviderSetupEvent::Site(sites.remove(0))));
                } else {
                    self.chat_widget.show_selection_view(SelectionViewParams {
                        title: Some("Select Provider Site".into()),
                        subtitle: Some("Each site uses a separate API key.".into()),
                        items: sites.into_iter().map(site_item).collect(),
                        ..Default::default()
                    });
                }
            }
            ProviderSetupEvent::Site(provider) => {
                if provider.conflict {
                    self.chat_widget.add_error_message("This provider ID has a custom configuration. Rename the custom entry before using built-in setup.".into());
                } else if provider.configured {
                    self.app_event_tx
                        .send(AppEvent::from(ProviderSetupEvent::Models {
                            provider,
                            mode: ModelCatalogMode::Recommended,
                        }));
                } else {
                    self.chat_widget
                        .show_bottom_pane_view(Box::new(ProviderKeyView::new(provider, tx)));
                }
            }
            ProviderSetupEvent::Key(provider) => self
                .chat_widget
                .show_bottom_pane_view(Box::new(ProviderKeyView::new(provider, tx))),
            ProviderSetupEvent::Prepare { provider, params } => {
                let items = provider
                    .recommended_models
                    .iter()
                    .enumerate()
                    .map(|(index, model)| {
                        let mut params = params.clone();
                        params.model = Some(model.clone());
                        let provider = provider.clone();
                        SelectionItem {
                            name: model.clone(),
                            is_default: index == 0,
                            actions: vec![Box::new(move |tx| {
                                tx.send(AppEvent::from(ProviderSetupEvent::Configure {
                                    provider: provider.clone(),
                                    params: params.clone(),
                                }))
                            })],
                            dismiss_on_select: true,
                            ..Default::default()
                        }
                    })
                    .collect();
                self.chat_widget.show_selection_view(SelectionViewParams { title: Some("Select a model to validate".into()),
                    subtitle: Some("A short test request may incur a small API charge. Esc cancels without saving.".into()), items, ..Default::default() });
            }
            ProviderSetupEvent::Configure { provider, params } => {
                self.chat_widget.add_info_message(
                    "Validating the provider with a short test request…".into(),
                    None,
                );
                tokio::spawn(async move {
                    let result = handle
                        .request_typed::<ModelProviderConfigureResponse>(
                            ClientRequest::ModelProviderConfigure {
                                request_id: request_id(),
                                params,
                            },
                        )
                        .await
                        .map_err(|error| error.to_string());
                    tx.send(AppEvent::from(ProviderSetupEvent::Configured(
                        thread_id, provider, result,
                    )));
                });
            }
            ProviderSetupEvent::Configured(origin, mut provider, result) if origin == thread_id => {
                match result {
                    Ok(response) => {
                        provider.configured = true;
                        self.app_event_tx
                            .send(AppEvent::from(ProviderSetupEvent::Select {
                                provider,
                                model: response.model,
                                effort: None,
                            }));
                    }
                    Err(error) => {
                        self.chat_widget.add_error_message(error);
                        self.chat_widget
                            .show_bottom_pane_view(Box::new(ProviderKeyView::new(provider, tx)));
                    }
                }
            }
            ProviderSetupEvent::Models { provider, mode } => {
                let catalog_id = uuid::Uuid::new_v4();
                self.provider_selection.catalog_request =
                    Some((thread_id, provider.id.clone(), catalog_id));
                tokio::spawn(async move {
                    let result = async {
                        let mut models = Vec::new();
                        let mut cursor = None;
                        let mut refresh = true;
                        loop {
                            let response = handle
                                .request_typed::<ModelListResponse>(ClientRequest::ModelList {
                                    request_id: request_id(),
                                    params: ModelListParams {
                                        provider_id: Some(provider.id.clone()),
                                        catalog_mode: Some(mode),
                                        refresh: Some(refresh && cursor.is_none()),
                                        cursor: cursor.clone(),
                                        limit: Some(500),
                                        include_hidden: None,
                                    },
                                })
                                .await;
                            let response = match response {
                                Ok(response) => response,
                                Err(error) if refresh && mode == ModelCatalogMode::All => {
                                    tx.send(AppEvent::from(ProviderSetupEvent::CatalogWarning(
                                        thread_id,
                                        format!("{error}. Showing cached and recommended models."),
                                    )));
                                    refresh = false;
                                    continue;
                                }
                                Err(error) => return Err(error.to_string()),
                            };
                            models.extend(response.data);
                            cursor = response.next_cursor;
                            if cursor.is_none() || models.len() >= 4096 {
                                break;
                            }
                        }
                        Ok(models)
                    }
                    .await;
                    tx.send(AppEvent::from(ProviderSetupEvent::ModelsLoaded(
                        thread_id, catalog_id, provider, result,
                    )));
                });
            }
            ProviderSetupEvent::ModelsLoaded(origin, catalog_id, provider, result)
                if origin == thread_id
                    && self.provider_selection.catalog_request.as_ref()
                        == Some(&(origin, provider.id.clone(), catalog_id)) =>
            {
                self.provider_selection.catalog_request = None;
                match result {
                    Ok(models) => {
                        let mut items: Vec<SelectionItem> = models
                            .into_iter()
                            .map(|model| {
                                let provider = provider.clone();
                                SelectionItem {
                                    name: model.display_name.clone(),
                                    description: Some(model.model.clone()),
                                    search_value: Some(format!(
                                        "{} {}",
                                        model.display_name, model.model
                                    )),
                                    actions: vec![Box::new(move |tx| {
                                        tx.send(AppEvent::from(ProviderSetupEvent::Choose(
                                            provider.clone(),
                                            Box::new(model.clone()),
                                        )))
                                    })],
                                    dismiss_on_select: true,
                                    ..Default::default()
                                }
                            })
                            .collect();
                        if provider.supports_model_discovery {
                            let p = provider.clone();
                            items.push(SelectionItem {
                                name: "Load more models".into(),
                                actions: vec![Box::new(move |tx| {
                                    tx.send(AppEvent::from(ProviderSetupEvent::Models {
                                        provider: p.clone(),
                                        mode: ModelCatalogMode::All,
                                    }))
                                })],
                                dismiss_on_select: true,
                                ..Default::default()
                            });
                        }
                        if provider.id != "openai" {
                            let p = provider.clone();
                            items.push(SelectionItem {
                                name: "Update API key".into(),
                                actions: vec![Box::new(move |tx| {
                                    tx.send(AppEvent::from(ProviderSetupEvent::Key(p.clone())))
                                })],
                                dismiss_on_select: true,
                                ..Default::default()
                            });
                            let p = provider.clone();
                            items.push(SelectionItem {
                                name: "Remove saved API key".into(),
                                actions: vec![Box::new(move |tx| {
                                    tx.send(AppEvent::from(ProviderSetupEvent::Delete(p.clone())))
                                })],
                                dismiss_on_select: true,
                                ..Default::default()
                            });
                        }
                        self.chat_widget.show_selection_view(SelectionViewParams { title: Some(format!("{} Models", provider.name)),
                            subtitle: (!provider.supports_model_discovery && provider.id != "openai").then(|| "This site has no supported model directory; showing built-in recommendations.".into()),
                            items, is_searchable: true, ..Default::default() });
                    }
                    Err(error) => self.chat_widget.add_error_message(format!(
                        "{error}. Reopen the provider to use recommended models."
                    )),
                }
            }
            ProviderSetupEvent::Choose(provider, model) => {
                let model = *model;
                if model.supported_reasoning_efforts.is_empty() {
                    self.app_event_tx
                        .send(AppEvent::from(ProviderSetupEvent::Select {
                            provider,
                            model: model.model,
                            effort: None,
                        }));
                } else {
                    let items = model
                        .supported_reasoning_efforts
                        .into_iter()
                        .map(|option| {
                            let provider = provider.clone();
                            let model_id = model.model.clone();
                            SelectionItem {
                                name: option.reasoning_effort.to_string(),
                                description: Some(option.description),
                                is_default: option.reasoning_effort
                                    == model.default_reasoning_effort,
                                actions: vec![Box::new(move |tx| {
                                    tx.send(AppEvent::from(ProviderSetupEvent::Select {
                                        provider: provider.clone(),
                                        model: model_id.clone(),
                                        effort: Some(option.reasoning_effort.clone()),
                                    }))
                                })],
                                dismiss_on_select: true,
                                ..Default::default()
                            }
                        })
                        .collect();
                    self.chat_widget.show_selection_view(SelectionViewParams {
                        title: Some("Select Reasoning Effort".into()),
                        items,
                        ..Default::default()
                    });
                }
            }
            ProviderSetupEvent::Select {
                provider,
                model,
                effort,
            } => {
                self.provider_selection.catalog_request = None;
                let selection_id = uuid::Uuid::new_v4();
                self.provider_selection
                    .latest
                    .insert(thread_id, selection_id);
                if let Some(thread_id) = thread_id {
                    self.provider_selection
                        .pending
                        .insert(thread_id, selection_id);
                    self.pending_model_selections.insert(
                        thread_id,
                        codex_app_server_protocol::ModelSelection {
                            model_provider: provider.id.clone(),
                            model: model.clone(),
                        },
                    );
                }
                self.chat_widget
                    .set_queue_autosend_suppressed(/*suppressed*/ true);
                self.chat_widget.add_info_message(
                    format!("Selecting {} / {model} for the next turn…", provider.name),
                    None,
                );
                let previous = self.provider_selection.task.take();
                let collaboration_mode = self
                    .chat_widget
                    .effective_collaboration_mode()
                    .with_updates(Some(model.clone()), Some(effort.clone()), None);
                self.provider_selection.task = Some(tokio::spawn(async move {
                    if let Some(previous) = previous {
                        let _ = previous.await;
                    }
                    let result = async {
                        if let Some(thread_id) = thread_id {
                            handle
                                .request_typed::<ThreadSettingsUpdateResponse>(
                                    ClientRequest::ThreadSettingsUpdate {
                                        request_id: request_id(),
                                        params: ThreadSettingsUpdateParams {
                                            thread_id: thread_id.to_string(),
                                            model_provider: Some(provider.id.clone()),
                                            collaboration_mode: Some(collaboration_mode),
                                            model: Some(model.clone()),
                                            effort: effort.clone(),
                                            ..Default::default()
                                        },
                                    },
                                )
                                .await
                                .map_err(|error| error.to_string())?;
                        }
                        let response = crate::config_update::write_config_batch(
                            handle,
                            vec![
                                crate::config_update::replace_config_value(
                                    "model_provider",
                                    json!(provider.id),
                                ),
                                crate::config_update::replace_config_value("model", json!(model)),
                                crate::config_update::replace_config_value(
                                    "model_reasoning_effort",
                                    json!(effort),
                                ),
                                crate::config_update::clear_config_value("service_tier"),
                            ],
                        )
                        .await;
                        let response = match response {
                            Ok(response) => response,
                            Err(error) => {
                                tx.send(AppEvent::from(ProviderSetupEvent::CatalogWarning(thread_id, format!("Provider selected for this task, but saving global defaults failed: {error}"))));
                                return Ok(WriteStatus::Ok);
                            }
                        };
                        Ok(response.status)
                    }
                    .await;
                    tx.send(AppEvent::from(ProviderSetupEvent::Selected(
                        thread_id,
                        selection_id,
                        provider.id,
                        model,
                        result,
                    )));
                }));
            }
            ProviderSetupEvent::Selected(origin, selection_id, provider, model, result) => {
                if self.provider_selection.latest.get(&origin) != Some(&selection_id) {
                    return;
                }
                if let Some(origin) = origin {
                    self.provider_selection.pending.remove(&origin);
                }
                if origin != thread_id {
                    return;
                }
                match result {
                    Ok(status) => {
                        self.app_event_tx.send(AppEvent::SettingsSelectionSettled);
                        if origin.is_some() || status != WriteStatus::OkOverridden {
                            self.config.model_provider_id = provider.clone();
                            if let Some(info) = self.config.model_providers.get(&provider) {
                                self.config.model_provider = info.clone();
                            }
                            self.config.model = Some(model.clone());
                        }
                        self.chat_widget.add_info_message(
                            format!("Selected {model}. The next turn uses this provider."),
                            None,
                        );
                        if status == WriteStatus::OkOverridden {
                            self.chat_widget.add_warning_message("Saved defaults are overridden by a higher-priority configuration layer.".into());
                        }
                        tokio::spawn(async move {
                            let result = handle
                                .request_typed::<ModelListResponse>(ClientRequest::ModelList {
                                    request_id: request_id(),
                                    params: ModelListParams {
                                        provider_id: Some(provider.clone()),
                                        catalog_mode: Some(ModelCatalogMode::All),
                                        refresh: Some(false),
                                        limit: Some(500),
                                        ..Default::default()
                                    },
                                })
                                .await
                                .map(|response| response.data)
                                .map_err(|error| error.to_string());
                            tx.send(AppEvent::from(ProviderSetupEvent::CatalogReloaded(
                                thread_id,
                                selection_id,
                                provider,
                                result,
                            )));
                        });
                    }
                    Err(error) => self.chat_widget.add_error_message(error),
                }
            }
            ProviderSetupEvent::Delete(provider) => {
                tokio::spawn(async move {
                    let result = handle
                        .request_typed::<ModelProviderCredentialDeleteResponse>(
                            ClientRequest::ModelProviderCredentialDelete {
                                request_id: request_id(),
                                params: ModelProviderCredentialDeleteParams {
                                    provider_id: provider.id,
                                },
                            },
                        )
                        .await
                        .map(|_| ())
                        .map_err(|error| error.to_string());
                    tx.send(AppEvent::from(ProviderSetupEvent::Deleted(
                        thread_id, result,
                    )));
                });
            }
            ProviderSetupEvent::Deleted(origin, result) if origin == thread_id => match result {
                Ok(()) => self.chat_widget.add_info_message(
                    "Saved API key removed. Connect again before using this provider.".into(),
                    None,
                ),
                Err(error) => self.chat_widget.add_error_message(error),
            },
            ProviderSetupEvent::CatalogReloaded(origin, selection_id, provider, result)
                if origin == thread_id
                    && self.provider_selection.latest.get(&origin) == Some(&selection_id)
                    && self.config.model_provider_id == provider =>
            {
                if let Ok(models) = result {
                    let models = models
                        .into_iter()
                        .map(crate::app_server_session::model_preset_from_api_model)
                        .collect();
                    self.chat_widget.replace_provider_catalog(models);
                    self.model_catalog = self.chat_widget.model_catalog();
                    app_server.set_available_models(self.model_catalog.models.clone());
                }
            }
            ProviderSetupEvent::CatalogWarning(origin, message) if origin == thread_id => {
                self.chat_widget.add_warning_message(message)
            }
            ProviderSetupEvent::Listed(..)
            | ProviderSetupEvent::Configured(..)
            | ProviderSetupEvent::ModelsLoaded(..)
            | ProviderSetupEvent::Deleted(..)
            | ProviderSetupEvent::CatalogReloaded(..)
            | ProviderSetupEvent::CatalogWarning(..) => {}
        }
    }
}

fn site_item(provider: ModelProviderInfo) -> SelectionItem {
    SelectionItem {
        name: provider.name.clone(),
        description: Some(
            if provider.conflict {
                "Custom configuration"
            } else if provider.configured {
                "Connected"
            } else {
                "Paste an API key to connect"
            }
            .into(),
        ),
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::from(ProviderSetupEvent::Site(provider.clone())))
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}
