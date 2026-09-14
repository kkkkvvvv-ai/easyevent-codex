//! Request-only history projection. Persisted envelopes are never rewritten here.
use super::ContextManager;
use codex_history::ResponseItemEnvelope;
use codex_model_provider::ProviderCapabilities;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;
use codex_protocol::models::AgentMessageInputContent;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::protocol::ModelOutputSource;
use std::collections::HashMap;
use std::collections::HashSet;

impl ContextManager {
    pub(crate) fn for_model_source_annotated(
        self,
        source: &ModelOutputSource,
        model: &ModelInfo,
        capabilities: ProviderCapabilities,
    ) -> Result<Vec<ResponseItemEnvelope>> {
        project_annotated(
            self.for_prompt_annotated(&model.input_modalities),
            source,
            capabilities,
            model.comp_hash.as_deref(),
        )
    }

    pub(crate) fn for_model_source(
        self,
        source: &ModelOutputSource,
        model: &ModelInfo,
        capabilities: ProviderCapabilities,
    ) -> Result<Vec<ResponseItem>> {
        project(
            self.for_prompt_annotated(&model.input_modalities),
            source,
            capabilities,
            model.comp_hash.as_deref(),
        )
    }
}

fn project(
    envelopes: Vec<ResponseItemEnvelope>,
    target: &ModelOutputSource,
    capabilities: ProviderCapabilities,
    compaction_hash: Option<&str>,
) -> Result<Vec<ResponseItem>> {
    Ok(
        project_annotated(envelopes, target, capabilities, compaction_hash)?
            .into_iter()
            .map(|envelope| envelope.item)
            .collect(),
    )
}

fn project_annotated(
    envelopes: Vec<ResponseItemEnvelope>,
    target: &ModelOutputSource,
    capabilities: ProviderCapabilities,
    compaction_hash: Option<&str>,
) -> Result<Vec<ResponseItemEnvelope>> {
    let mut result = Vec::with_capacity(envelopes.len());
    let mut calls = HashMap::new();
    let mut used_ids: HashSet<String> = envelopes
        .iter()
        .filter_map(|envelope| match &envelope.item {
            ResponseItem::FunctionCall { call_id, .. }
            | ResponseItem::CustomToolCall { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    for (index, envelope) in envelopes.into_iter().enumerate() {
        let source = envelope
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.model_source.as_ref());
        let same_source = source == Some(target);
        let same_runtime = source.is_some_and(|source| {
            source.provider_id == target.provider_id && source.identity == target.identity
        });
        let same_compaction = same_runtime
            && (same_source
                || envelope
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.compaction_model_hash.as_deref())
                    .zip(compaction_hash)
                    .is_some_and(|(old, new)| old == new));
        let mut item = envelope.item;
        match &mut item {
            ResponseItem::Reasoning { .. } if !same_source => continue,
            ResponseItem::ConfigurationUpdate { .. } | ResponseItem::CompactionTrigger { .. }
                if !same_runtime =>
            {
                continue;
            }
            ResponseItem::Compaction { .. }
            | ResponseItem::ContextCompaction {
                encrypted_content: Some(_),
                ..
            } if !same_compaction => return Err(handoff_required()),
            ResponseItem::FunctionCall {
                encrypted_function_args: Some(args),
                ..
            } if (!same_runtime) && !args.is_empty() => {
                return Err(handoff_required());
            }
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. }
                if !same_runtime =>
            {
                if let FunctionCallOutputBody::ContentItems(items) = &output.body
                    && items.iter().any(|item| {
                        matches!(item, FunctionCallOutputContentItem::EncryptedContent { .. })
                    })
                {
                    return Err(handoff_required());
                }
            }
            ResponseItem::AgentMessage {
                author,
                recipient,
                content,
                ..
            } if !same_runtime => {
                let mut content = content
                    .iter()
                    .map(|part| match part {
                        AgentMessageInputContent::InputText { text } => {
                            Ok(ContentItem::InputText { text: text.clone() })
                        }
                        AgentMessageInputContent::EncryptedContent { .. } => {
                            Err(handoff_required())
                        }
                    })
                    .collect::<Result<Vec<_>>>()?;
                content.insert(
                    0,
                    ContentItem::InputText {
                        text: format!("Agent message from {author} to {recipient}:\n"),
                    },
                );
                item = ResponseItem::Message {
                    id: None,
                    role: "user".into(),
                    content,
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                };
            }
            ResponseItem::CustomToolCall { .. }
            | ResponseItem::CustomToolCallOutput { .. }
            | ResponseItem::FunctionCall {
                namespace: Some(_), ..
            }
            | ResponseItem::FunctionCallOutput {
                namespace: Some(_), ..
            } if !same_runtime && !capabilities.namespace_tools => {
                return Err(handoff_required());
            }
            ResponseItem::AdditionalTools { .. }
            | ResponseItem::LocalShellCall { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::ToolSearchOutput { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::ImageGenerationCall { .. }
                if !same_runtime =>
            {
                return Err(handoff_required());
            }
            _ => {}
        }
        let host_message =
            matches!(&item, ResponseItem::Message { role, .. } if role != "assistant");
        if !same_runtime && !host_message {
            item.set_id(None);
            item.clear_internal_chat_message_metadata_passthrough();
        }
        match &mut item {
            ResponseItem::FunctionCall { call_id, .. }
            | ResponseItem::CustomToolCall { call_id, .. } => {
                let original = call_id.clone();
                if !same_runtime || calls.contains_key(&original) {
                    let mut candidate = format!("codex_history_{index}");
                    while !used_ids.insert(candidate.clone()) {
                        candidate.push('_');
                    }
                    *call_id = candidate;
                }
                calls.insert(original, call_id.clone());
            }
            ResponseItem::FunctionCallOutput {
                call_id: Some(call_id),
                ..
            }
            | ResponseItem::CustomToolCallOutput { call_id, .. } => {
                if let Some(mapped) = calls.get(call_id) {
                    call_id.clone_from(mapped);
                }
            }
            _ => {}
        }
        result.push(ResponseItemEnvelope {
            item,
            metadata: envelope.metadata,
        });
    }
    Ok(result)
}

fn handoff_required() -> CodexErr {
    CodexErr::InvalidRequest("provider switch requires a readable history handoff".into())
}

#[cfg(test)]
#[path = "provider_view_tests.rs"]
mod tests;
