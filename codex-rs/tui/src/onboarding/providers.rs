//! Hosted provider onboarding before an OpenAI account exists.

// Match auth.rs: poisoned onboarding state means the UI task has already failed.
#![expect(
    clippy::unwrap_used,
    reason = "onboarding shares the auth widget's poison-fatal state locks"
)]

use super::auth::AuthModeWidget;
use super::auth::SignInState;
use super::auth::onboarding_request_id;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ModelProviderConfigureParams;
use codex_app_server_protocol::ModelProviderConfigureResponse;
use codex_app_server_protocol::ModelProviderInfo;
use codex_app_server_protocol::ModelProviderListParams;
use codex_app_server_protocol::ModelProviderListResponse;
use codex_app_server_protocol::WriteStatus;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use serde_json::json;
use uuid::Uuid;

#[derive(Clone)]
pub(super) enum ProviderOnboardingState {
    Loading(Uuid),
    Sites {
        data: Vec<ModelProviderInfo>,
        selected: usize,
    },
    Key {
        provider: ModelProviderInfo,
        value: String,
    },
    Model {
        provider: ModelProviderInfo,
        key: Option<String>,
        selected: usize,
    },
    Verifying(Uuid),
}

impl ProviderOnboardingState {
    pub(super) fn render(&self, area: Rect, buf: &mut Buffer, error: Option<String>) {
        let mut lines: Vec<Line> = vec!["Connect a model provider".bold().into(), "".into()];
        match self {
            Self::Loading(_) => lines.push("Loading provider sites…".into()),
            Self::Verifying(_) => {
                lines.push("Validating Responses access and saving the key…".into())
            }
            Self::Sites { data, selected } => {
                for (index, provider) in data.iter().enumerate() {
                    let line = Line::from(format!(
                        "{} {}{}",
                        if index == *selected { ">" } else { " " },
                        provider.name,
                        if provider.configured {
                            " (connected)"
                        } else {
                            ""
                        }
                    ));
                    lines.push(if index == *selected {
                        line.cyan()
                    } else {
                        line
                    });
                }
                lines.push("↑/↓ select · Enter continue · Esc back".dim().into());
            }
            Self::Key { provider, value } => {
                lines.push(provider.name.clone().into());
                lines.push(provider.key_instructions.clone().into());
                lines.push(provider.base_url.clone().dim().into());
                lines
                    .push(format!("API key: {}", "•".repeat(value.chars().count().min(60))).into());
                lines.push(
                    "Enter selects a model to validate. Esc cancels without saving."
                        .dim()
                        .into(),
                );
            }
            Self::Model {
                provider, selected, ..
            } => {
                for (index, model) in provider.recommended_models.iter().enumerate() {
                    lines.push(
                        format!("{} {model}", if index == *selected { ">" } else { " " }).into(),
                    );
                }
                lines.push(
                    "↑/↓ select · Enter validate and use · Esc cancel"
                        .dim()
                        .into(),
                );
                lines.push(
                    "Validation sends a short test prompt and may incur a small API charge."
                        .dim()
                        .into(),
                );
            }
        }
        if let Some(error) = error {
            lines.push(error.red().into());
        }
        Paragraph::new(crate::wrapping::word_wrap_lines(
            lines,
            usize::from(area.width),
        ))
        .render(area, buf);
    }
}

impl AuthModeWidget {
    pub(super) fn start_provider_setup(&mut self) {
        let generation = Uuid::new_v4();
        *self.sign_in_state.write().unwrap() =
            SignInState::Provider(ProviderOnboardingState::Loading(generation));
        let handle = self.app_server_request_handle.clone();
        let state = self.sign_in_state.clone();
        let error = self.error.clone();
        let frame = self.request_frame.clone();
        tokio::spawn(async move {
            let result = handle
                .request_typed::<ModelProviderListResponse>(ClientRequest::ModelProviderList {
                    request_id: onboarding_request_id(),
                    params: ModelProviderListParams::default(),
                })
                .await;
            let mut state = state.write().unwrap();
            if matches!(&*state, SignInState::Provider(ProviderOnboardingState::Loading(id)) if *id == generation)
            {
                match result {
                    Ok(response) => {
                        *state = SignInState::Provider(ProviderOnboardingState::Sites {
                            data: response.data,
                            selected: 0,
                        })
                    }
                    Err(err) => {
                        *error.write().unwrap() = Some(err.to_string());
                        *state = SignInState::PickMode;
                    }
                }
            }
            frame.schedule_frame();
        });
    }

    pub(super) fn handle_provider_key(&mut self, event: KeyEvent) -> bool {
        let state = self.sign_in_state.read().unwrap().clone();
        let SignInState::Provider(mut state) = state else {
            return false;
        };
        if event.kind == KeyEventKind::Release {
            return true;
        }
        if event.code == KeyCode::Esc {
            // Submitted validation owns its credential write; don't claim to cancel it.
            if !matches!(state, ProviderOnboardingState::Verifying(_)) {
                *self.sign_in_state.write().unwrap() = SignInState::PickMode;
            }
            return true;
        }
        match &mut state {
            ProviderOnboardingState::Sites { data, selected } if !data.is_empty() => {
                match event.code {
                    KeyCode::Up => *selected = (*selected + data.len() - 1) % data.len(),
                    KeyCode::Down => *selected = (*selected + 1) % data.len(),
                    KeyCode::Enter => {
                        let provider = data[*selected].clone();
                        if provider.conflict {
                            *self.error.write().unwrap() = Some(
                                "Rename the conflicting custom provider entry before connecting."
                                    .into(),
                            );
                        } else if provider.configured {
                            state = ProviderOnboardingState::Model {
                                provider,
                                key: None,
                                selected: 0,
                            };
                        } else {
                            state = ProviderOnboardingState::Key {
                                provider,
                                value: String::new(),
                            };
                        }
                    }
                    _ => {}
                }
            }
            ProviderOnboardingState::Key { provider, value } => match event.code {
                KeyCode::Backspace => {
                    value.pop();
                }
                KeyCode::Char(character)
                    if !event.modifiers.intersects(
                        crossterm::event::KeyModifiers::CONTROL
                            | crossterm::event::KeyModifiers::ALT,
                    ) && !character.is_control()
                        && value.len() < 4096 =>
                {
                    value.push(character)
                }
                KeyCode::Enter if !value.trim().is_empty() => {
                    state = ProviderOnboardingState::Model {
                        provider: provider.clone(),
                        key: Some(std::mem::take(value)),
                        selected: 0,
                    };
                }
                _ => {}
            },
            ProviderOnboardingState::Model {
                provider,
                key,
                selected,
            } if !provider.recommended_models.is_empty() => match event.code {
                KeyCode::Up => {
                    *selected = (*selected + provider.recommended_models.len() - 1)
                        % provider.recommended_models.len()
                }
                KeyCode::Down => *selected = (*selected + 1) % provider.recommended_models.len(),
                KeyCode::Enter => {
                    self.save_provider(
                        provider.clone(),
                        key.take(),
                        provider.recommended_models[*selected].clone(),
                    );
                    return true;
                }
                _ => {}
            },
            _ => {}
        }
        *self.sign_in_state.write().unwrap() = SignInState::Provider(state);
        true
    }

    pub(super) fn handle_provider_paste(&mut self, pasted: &str) {
        if let SignInState::Provider(ProviderOnboardingState::Key { value, .. }) =
            &mut *self.sign_in_state.write().unwrap()
            && pasted.trim().len() <= 4096
            && !pasted.trim().chars().any(char::is_control)
        {
            *value = pasted.trim().to_string();
        }
    }

    fn save_provider(&mut self, provider: ModelProviderInfo, key: Option<String>, model: String) {
        let generation = Uuid::new_v4();
        *self.sign_in_state.write().unwrap() =
            SignInState::Provider(ProviderOnboardingState::Verifying(generation));
        let handle = self.app_server_request_handle.clone();
        let state = self.sign_in_state.clone();
        let error = self.error.clone();
        let frame = self.request_frame.clone();
        tokio::spawn(async move {
            let result = async {
                if let Some(key) = key {
                    handle
                        .request_typed::<ModelProviderConfigureResponse>(
                            ClientRequest::ModelProviderConfigure {
                                request_id: onboarding_request_id(),
                                params: ModelProviderConfigureParams {
                                    provider_id: provider.id.clone(),
                                    api_key: key,
                                    model: Some(model.clone()),
                                },
                            },
                        )
                        .await
                        .map_err(|err| err.to_string())?;
                }
                let response = crate::config_update::write_config_batch(
                    handle,
                    vec![
                        crate::config_update::replace_config_value(
                            "model_provider",
                            json!(provider.id),
                        ),
                        crate::config_update::replace_config_value("model", json!(model)),
                        crate::config_update::clear_config_value("model_reasoning_effort"),
                        crate::config_update::clear_config_value("service_tier"),
                    ],
                )
                .await
                .map_err(|err| err.to_string())?;
                if response.status == WriteStatus::OkOverridden {
                    return Err(
                        "Provider selection is overridden by a higher-priority configuration"
                            .to_string(),
                    );
                }
                Ok(())
            }
            .await;
            let mut state = state.write().unwrap();
            if matches!(&*state, SignInState::Provider(ProviderOnboardingState::Verifying(id)) if *id == generation)
            {
                match result {
                    Ok(()) => {
                        *state = SignInState::ProviderConfigured;
                        *error.write().unwrap() = None;
                    }
                    Err(err) => {
                        *state = SignInState::Provider(ProviderOnboardingState::Key {
                            provider,
                            value: String::new(),
                        });
                        *error.write().unwrap() = Some(err);
                    }
                }
            }
            frame.schedule_frame();
        });
    }
}

#[cfg(test)]
#[path = "providers_tests.rs"]
mod tests;
