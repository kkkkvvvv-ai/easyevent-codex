//! A masked provider key field that never participates in composer history.

use super::BottomPaneView;
use super::CancellationEvent;
use crate::app::providers::ProviderSetupEvent;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::render::renderable::Renderable;
use codex_app_server_protocol::ModelProviderConfigureParams;
use codex_app_server_protocol::ModelProviderInfo;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

pub(crate) struct ProviderKeyView {
    provider: ModelProviderInfo,
    value: String,
    complete: bool,
    tx: AppEventSender,
}

impl ProviderKeyView {
    pub(crate) fn new(provider: ModelProviderInfo, tx: AppEventSender) -> Self {
        Self {
            provider,
            value: String::new(),
            complete: false,
            tx,
        }
    }

    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = vec![format!("Connect {}", self.provider.name).bold().into()];
        lines.extend(
            textwrap::wrap(&self.provider.key_instructions, usize::from(width.max(1)))
                .into_iter()
                .map(|line| Line::from(line.into_owned())),
        );
        lines.push(self.provider.base_url.clone().dim().into());
        lines.push(Line::from(format!(
            "API key: {}",
            "•".repeat(self.value.chars().count().min(60))
        )));
        lines.push("Enter: choose model · Esc: cancel".dim().into());
        lines.extend(textwrap::wrap("Validation sends a short test prompt and may incur a small API charge. Your conversation is not sent.", usize::from(width.max(1)))
            .into_iter().map(|line| Line::from(line.into_owned()).dim()));
        lines
    }
}

impl BottomPaneView for ProviderKeyView {
    fn handle_key_event(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.value.clear();
                self.complete = true;
            }
            KeyCode::Backspace => {
                self.value.pop();
            }
            KeyCode::Char(character)
                if !key.modifiers.intersects(
                    crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::ALT,
                ) && !character.is_control()
                    && self.value.len() < 4096 =>
            {
                self.value.push(character)
            }
            KeyCode::Enter if !self.value.trim().is_empty() => {
                self.complete = true;
                self.tx.send(AppEvent::from(ProviderSetupEvent::Prepare {
                    provider: self.provider.clone(),
                    params: ModelProviderConfigureParams {
                        provider_id: self.provider.id.clone(),
                        api_key: std::mem::take(&mut self.value),
                        model: None,
                    },
                }));
            }
            _ => {}
        }
    }
    fn is_complete(&self) -> bool {
        self.complete
    }
    fn handle_paste(&mut self, value: String) -> bool {
        if value.trim().len() <= 4096 && !value.trim().chars().any(char::is_control) {
            self.value = value.trim().to_string();
        }
        true
    }
    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.value.clear();
        self.complete = true;
        CancellationEvent::Handled
    }
}

impl Renderable for ProviderKeyView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(self.lines(area.width)).render(area, buf);
    }
    fn desired_height(&self, width: u16) -> u16 {
        self.lines(width).len() as u16
    }
}

#[cfg(test)]
#[path = "provider_key_view_tests.rs"]
mod tests;
