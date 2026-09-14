//! Resume model defaults from surviving rollout segments, including choices made between turns.
//! Kept beside core's rollout replay because user boundaries use its contextual-fragment parser.
use crate::context_manager::is_user_turn_boundary;
use codex_history::RolloutItem;
use codex_protocol::ThreadId;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ThreadSettingsSnapshot;

/// A persisted next-turn selection. Provider IDs and raw model IDs remain separate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedModelSelection {
    pub model_provider: String,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
}

#[derive(Default)]
struct Segment<'a> {
    turn_id: Option<String>,
    user_turn: bool,
    selection: Option<&'a RolloutItem>,
    settings_event: bool,
}

impl<'a> Segment<'a> {
    fn finish(&mut self, pending_rollback: &mut usize) -> Option<&'a RolloutItem> {
        let segment = std::mem::take(self);
        if *pending_rollback > 0 {
            if segment.user_turn {
                *pending_rollback -= 1;
            }
            None
        } else {
            segment.selection
        }
    }
}

/// Replays the same reverse turn segments as model-history reconstruction. Settings from deleted
/// turns cannot override surviving defaults; a choice made after the last turn remains restorable.
pub fn latest_persisted_model_selection(
    history: &[RolloutItem],
) -> Option<PersistedModelSelection> {
    let legacy_provider = history
        .iter()
        .find_map(|item| match item {
            RolloutItem::SessionMeta(meta) => {
                Some(meta.meta.model_provider.as_deref().unwrap_or("openai"))
            }
            _ => None,
        })
        .unwrap_or("openai");
    latest_surviving_selection(history, SelectionScope::ModelDefaults).map(|item| match item {
        RolloutItem::EventMsg(EventMsg::ThreadSettingsApplied(event)) => PersistedModelSelection {
            model_provider: event.thread_settings.model_provider_id.clone(),
            model: event.thread_settings.collaboration_mode.model().to_owned(),
            reasoning_effort: event.thread_settings.collaboration_mode.reasoning_effort(),
        },
        RolloutItem::TurnContext(context) => PersistedModelSelection {
            model_provider: context
                .model_source
                .as_ref()
                .map_or(legacy_provider, |source| source.provider_id.as_str())
                .to_owned(),
            model: context.model.clone(),
            reasoning_effort: context.effort.clone(),
        },
        _ => unreachable!("selection must be a settings snapshot or turn context"),
    })
}

/// Reads only the requested thread's surviving settings, ignoring inherited and legacy owners.
pub(crate) fn latest_persisted_owned_thread_settings(
    history: &[RolloutItem],
    thread_id: ThreadId,
) -> Option<&ThreadSettingsSnapshot> {
    latest_surviving_selection(history, SelectionScope::OwnedSettings(thread_id)).map(|item| {
        let RolloutItem::EventMsg(EventMsg::ThreadSettingsApplied(event)) = item else {
            unreachable!("owned selection must be a settings snapshot");
        };
        &event.thread_settings
    })
}

#[derive(Clone, Copy)]
enum SelectionScope {
    ModelDefaults,
    OwnedSettings(ThreadId),
}

fn latest_surviving_selection(
    history: &[RolloutItem],
    scope: SelectionScope,
) -> Option<&RolloutItem> {
    let mut segment = Segment::default();
    let mut pending_rollback = 0usize;
    for item in history.iter().rev() {
        match item {
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(event)) => {
                // Settings appended after the rollback belong to the new surviving tail.
                if segment.turn_id.is_none() && !segment.user_turn && segment.settings_event {
                    return segment.selection;
                }
                if let Some(selection) = segment.finish(&mut pending_rollback) {
                    return Some(selection);
                }
                pending_rollback = pending_rollback.saturating_add(event.num_turns as usize);
            }
            RolloutItem::EventMsg(EventMsg::ThreadSettingsApplied(event))
                if !segment.settings_event
                    && match scope {
                        SelectionScope::ModelDefaults => true,
                        SelectionScope::OwnedSettings(owner) => event.thread_id == Some(owner),
                    } =>
            {
                // Older writers can append a frozen TurnContext after an acknowledged update.
                // Within one replay segment, its settings snapshot remains authoritative.
                segment.settings_event = true;
                segment.selection = Some(item);
            }
            RolloutItem::TurnContext(context) => {
                if let Some(turn_id) = &context.turn_id {
                    if segment
                        .turn_id
                        .as_ref()
                        .is_some_and(|current| current != turn_id)
                        && let Some(selection) = segment.finish(&mut pending_rollback)
                    {
                        return Some(selection);
                    }
                    segment.turn_id = Some(turn_id.clone());
                }
                if matches!(scope, SelectionScope::ModelDefaults) {
                    segment.selection.get_or_insert(item);
                }
            }
            RolloutItem::EventMsg(EventMsg::TurnComplete(event)) => {
                // A standalone choice made after completion is independent of the old turn.
                // Rollback removes choices made inside deleted turns, not later user defaults.
                if segment.turn_id.is_none() && !segment.user_turn && segment.settings_event {
                    return segment.selection;
                }
                segment.turn_id.get_or_insert_with(|| event.turn_id.clone());
            }
            RolloutItem::EventMsg(EventMsg::TurnAborted(event)) if segment.turn_id.is_none() => {
                segment.turn_id.clone_from(&event.turn_id);
            }
            RolloutItem::EventMsg(EventMsg::UserMessage(_))
            | RolloutItem::InterAgentCommunication(_) => {
                segment.user_turn = true;
            }
            RolloutItem::ResponseItem(item) => {
                segment.user_turn |= is_user_turn_boundary(&item.item);
            }
            RolloutItem::EventMsg(EventMsg::TurnStarted(event)) => {
                if segment
                    .turn_id
                    .as_ref()
                    .is_none_or(|current| current == &event.turn_id)
                    && let Some(selection) = segment.finish(&mut pending_rollback)
                {
                    return Some(selection);
                }
            }
            _ => {}
        }
    }
    segment.finish(&mut pending_rollback)
}

#[cfg(test)]
#[path = "thread_model_selection_tests.rs"]
mod tests;
