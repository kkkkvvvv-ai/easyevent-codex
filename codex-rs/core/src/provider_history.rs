//! Strip endpoint-owned opaque state from outbound history after a provider switch.
//! The persisted transcript is unchanged; ordinary messages and tool results remain.

use std::collections::HashSet;

use codex_history::RolloutItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use sha2::Digest;
use sha2::Sha256;

#[derive(Default, Clone)]
pub(crate) struct ProviderHistory {
    foreign: HashSet<[u8; 32]>,
}

impl std::fmt::Debug for ProviderHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderHistory")
            .field("foreign_items", &self.foreign.len())
            .finish()
    }
}

impl ProviderHistory {
    pub(crate) fn from_items<'a>(items: impl Iterator<Item = &'a ResponseItem>) -> Self {
        let mut result = Self::default();
        for item in items {
            result.record(item);
        }
        result
    }

    pub(crate) fn from_rollout(items: &[RolloutItem], target: &str) -> Self {
        let mut result = Self::default();
        let mut owner = target;
        for (index, item) in items.iter().enumerate() {
            match item {
                RolloutItem::SessionMeta(meta) => {
                    owner = meta.meta.model_provider.as_deref().unwrap_or(target)
                }
                RolloutItem::EventMsg(EventMsg::ThreadSettingsApplied(event)) => {
                    owner = &event.thread_settings.model_provider_id
                }
                RolloutItem::ResponseItem(envelope) if owner != target => {
                    result.record(&envelope.item)
                }
                RolloutItem::Compacted(compacted) => {
                    // A paginated resume may omit earlier settings. Compaction writes
                    // its current settings after the checkpoint and optional baselines.
                    let checkpoint_owner = items[index + 1..]
                        .iter()
                        .take_while(|item| {
                            matches!(
                                item,
                                RolloutItem::WorldState(_)
                                    | RolloutItem::TurnContext(_)
                                    | RolloutItem::EventMsg(EventMsg::ThreadSettingsApplied(_))
                            )
                        })
                        .find_map(|item| match item {
                            RolloutItem::EventMsg(EventMsg::ThreadSettingsApplied(event)) => {
                                Some(event.thread_settings.model_provider_id.as_str())
                            }
                            _ => None,
                        })
                        .unwrap_or(owner);
                    if checkpoint_owner != target
                        && let Some(history) = &compacted.replacement_history
                    {
                        for envelope in history {
                            result.record(&envelope.item);
                        }
                    }
                }
                _ => {}
            }
        }
        result
    }

    fn record(&mut self, item: &ResponseItem) {
        match item {
            ResponseItem::Reasoning {
                encrypted_content: Some(content),
                ..
            }
            | ResponseItem::ContextCompaction {
                encrypted_content: Some(content),
                ..
            }
            | ResponseItem::Compaction {
                encrypted_content: content,
                ..
            } => {
                self.foreign
                    .insert(Sha256::digest(content.as_bytes()).into());
            }
            ResponseItem::FunctionCall {
                encrypted_function_args: Some(args),
                ..
            } => {
                for arg in args {
                    self.foreign.insert(Sha256::digest(arg.as_bytes()).into());
                }
            }
            _ => {}
        }
    }

    pub(crate) fn filter(&self, items: &mut Vec<ResponseItem>) {
        let is_foreign = |text: &str| {
            self.foreign
                .contains(&<[u8; 32]>::from(Sha256::digest(text.as_bytes())))
        };
        items.retain_mut(|item| match item {
            ResponseItem::Reasoning {
                encrypted_content,
                summary,
                content,
                ..
            } if encrypted_content.as_deref().is_some_and(is_foreign) => {
                *encrypted_content = None;
                !summary.is_empty() || content.as_ref().is_some_and(|content| !content.is_empty())
            }
            ResponseItem::Compaction {
                encrypted_content, ..
            } => !is_foreign(encrypted_content),
            ResponseItem::ContextCompaction {
                encrypted_content: Some(content),
                ..
            } => !is_foreign(content),
            ResponseItem::FunctionCall {
                encrypted_function_args,
                ..
            } => {
                if encrypted_function_args
                    .as_ref()
                    .is_some_and(|args| args.iter().any(|arg| is_foreign(arg)))
                {
                    *encrypted_function_args = None;
                }
                true
            }
            _ => true,
        });
    }
}
