//! Prepare and durably checkpoint portable history before activating a target model.
use super::session::Session;
use super::step_context::StepContext;
use crate::client_common::Prompt;
use crate::client_common::ResponseEvent;
use crate::context::CompactionSummary;
use crate::context::ContextualUserFragment;
use crate::context::ProviderHandoffRequest;
use crate::context_manager::ContextManager;
use crate::context_manager::HistoryReplacement;
use crate::context_manager::estimate_item_token_count;
use crate::context_manager::is_user_turn_boundary;
use crate::responses_metadata::CodexResponsesMetadata;
use crate::responses_metadata::CodexResponsesRequestKind;
use codex_history::CompactedItem;
use codex_history::ResponseItemEnvelope;
use codex_history::RolloutItem;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::BaseInstructionsProvenance;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_rollout_trace::InferenceTraceContext;
use codex_utils_output_truncation::approx_token_count;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

impl Session {
    pub(crate) async fn provider_base_instructions(&self, step: &StepContext) -> BaseInstructions {
        let mut instructions = self.get_prompt_base_instructions().await;
        if matches!(
            instructions.provenance,
            Some(BaseInstructionsProvenance::Model { .. })
        ) && self.services.model_client.provider().info() != step.turn.provider.info()
        {
            instructions.text = step
                .settings
                .model_info
                .get_model_instructions(step.settings.personality());
            if !step.turn.config.update_plan_enabled {
                instructions.text =
                    crate::context::without_update_plan_instructions(&instructions.text);
            }
            instructions.provenance = Some(BaseInstructionsProvenance::Model {
                model: step.settings.model_info.slug.clone(),
            });
        }
        instructions
    }

    #[tracing::instrument(level = "trace", skip_all)]
    pub(super) async fn prepare_provider_handoff(
        &self,
        step: &StepContext,
        cancellation: &CancellationToken,
    ) -> Result<()> {
        let target = &step.turn;
        let model = &step.settings.model_info;
        let source = target.model_runtime.source_for(&model.slug)?;
        let (previous, settings, active) = {
            let state = self.state.lock().await;
            (
                state.active_model_runtime.clone(),
                state.active_model_settings.clone(),
                state.active_model_source.clone(),
            )
        };
        if active.as_ref() == Some(&source) {
            return Ok(());
        }
        let history = self.clone_history().await;
        let version = history.history_version();
        let instructions = self.provider_base_instructions(step).await;
        let metadata = self
            .responses_metadata(step, CodexResponsesRequestKind::Turn)
            .await;
        let window = model.usable_context_window();
        let budget = window.map(|window| {
            window
                .saturating_sub((window / 4).min(16384))
                .saturating_sub(256)
        });
        let fits = |input: Vec<ResponseItem>| -> Result<bool> {
            let prompt = super::turn::build_prompt(input, step, instructions.clone());
            let tokens = request_tokens(&prompt, step, &metadata)?;
            Ok(budget.is_none_or(|limit| tokens < limit))
        };
        let projected =
            history
                .clone()
                .for_model_source(&source, model, target.provider.capabilities());
        let native_continuation = active.is_none();
        if let Ok(items) = projected.as_ref()
            && (native_continuation || fits(items.clone())?)
        {
            return Ok(());
        }
        let (Some(previous), Some(settings)) = (previous, settings) else {
            return Err(CodexErr::InvalidRequest("The original provider is unavailable for the required history handoff. Reconnect it or select a model that can read and fit the saved history.".into()));
        };
        let old_source = previous.source_for(&settings.model_info.slug)?;
        let mut old_input = history.clone().for_model_source(
            &old_source,
            &settings.model_info,
            previous.provider.capabilities(),
        )?;
        let world_state = std::sync::Arc::new(self.build_world_state_for_step(step).await?);
        let initial_context = self
            .build_initial_context_with_world_state(step, &world_state)
            .await;
        let empty_tokens = request_tokens(
            &super::turn::build_prompt(initial_context.clone(), step, instructions.clone()),
            step,
            &metadata,
        )?;
        let summary_limit = budget.map_or(4096, |limit| {
            limit.saturating_sub(empty_tokens).clamp(0, 4096) as usize
        });
        if summary_limit == 0 {
            return Err(CodexErr::ContextWindowExceeded);
        }
        old_input.push(ContextualUserFragment::into(ProviderHandoffRequest {
            token_limit: summary_limit,
        }));
        let mut old_instructions = self.get_prompt_base_instructions().await;
        if matches!(
            old_instructions.provenance,
            Some(BaseInstructionsProvenance::Model { .. })
        ) {
            old_instructions.text = settings
                .model_info
                .get_model_instructions(settings.personality());
        }
        let prompt = Prompt {
            input: old_input,
            base_instructions: old_instructions,
            ..Default::default()
        };
        let summary_metadata = self
            .responses_metadata(
                step,
                CodexResponsesRequestKind::Compaction(
                    crate::responses_metadata::CompactionTurnMetadata::new(
                        codex_analytics::CompactionTrigger::Auto,
                        if projected.is_err() {
                            codex_analytics::CompactionReason::CompHashChanged
                        } else {
                            codex_analytics::CompactionReason::ModelDownshift
                        },
                        codex_analytics::CompactionImplementation::Responses,
                        codex_analytics::CompactionPhase::PreTurn,
                    ),
                ),
            )
            .await;
        let summary_request = async {
            let mut client = previous.client.new_session();
            let telemetry = settings.telemetry(&self.services.session_telemetry);
            let mut stream = client
                .stream(
                    &prompt,
                    &settings.model_info,
                    &telemetry,
                    settings.reasoning_effort().cloned(),
                    settings.reasoning_summary,
                    settings.service_tier.clone(),
                    &summary_metadata,
                    &InferenceTraceContext::disabled(),
                )
                .await?;
            let mut summary = String::new();
            while let Some(event) = stream.next().await {
                match event? {
                    ResponseEvent::OutputItemDone(ResponseItem::Message {
                        role, content, ..
                    }) if role == "assistant" => {
                        for part in content {
                            if let ContentItem::OutputText { text } = part {
                                if summary.len().saturating_add(text.len()) > 16384 {
                                    return Err(CodexErr::InvalidRequest(
                                        "Handoff summary exceeds the byte limit".into(),
                                    ));
                                }
                                summary.push_str(&text);
                                if approx_token_count(&summary) > summary_limit {
                                    return Err(CodexErr::InvalidRequest(
                                        "Handoff summary exceeds the token limit".into(),
                                    ));
                                }
                            }
                        }
                    }
                    ResponseEvent::OutputItemDone(ResponseItem::Reasoning { .. }) => {}
                    ResponseEvent::OutputItemDone(_) => {
                        return Err(CodexErr::InvalidRequest(
                            "Handoff returned an unexpected non-text item".into(),
                        ));
                    }
                    ResponseEvent::Completed { response_id, .. } => {
                        if summary.trim().is_empty() {
                            return Err(CodexErr::InvalidRequest(
                                "Handoff returned an empty summary".into(),
                            ));
                        }
                        return Ok((summary, response_id));
                    }
                    _ => {}
                }
            }
            Err(CodexErr::Stream(
                "Handoff ended before a complete response".into(),
            ))
        };
        let (summary, response_id) = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(CodexErr::TurnAborted),
            result = tokio::time::timeout(std::time::Duration::from_secs(/*secs*/ 60), summary_request) => result.map_err(|_| CodexErr::InvalidRequest("History handoff timed out".into()))??,
        };
        let summary_item = ResponseItemEnvelope {
            item: ContextualUserFragment::into(CompactionSummary::new(summary.clone())),
            metadata: Some(codex_history::CodexHarnessMetadata {
                model_source: Some(old_source),
                ..Default::default()
            }),
        };
        let items = history.annotated_items();
        let mut replacement = vec![summary_item.clone()];
        // Always retain the latest user turn, including the task just submitted.
        let mut kept_latest = false;
        for start in items
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(index, item)| is_user_turn_boundary(&item.item).then_some(index))
        {
            let mut candidate: Vec<ResponseItemEnvelope> = initial_context
                .iter()
                .cloned()
                .map(ResponseItemEnvelope::new)
                .collect();
            candidate.push(summary_item.clone());
            candidate.extend_from_slice(&items[start..]);
            let mut view = ContextManager::new();
            view.replace_annotated(candidate.clone());
            match view.for_model_source(&source, model, target.provider.capabilities()) {
                Ok(input) if fits(input.clone())? => {
                    replacement = candidate;
                    kept_latest = true;
                }
                _ => break,
            }
        }
        if !kept_latest || cancellation.is_cancelled() {
            return Err(CodexErr::InvalidRequest(
                "Target model cannot fit the handoff and latest complete user turn".into(),
            ));
        }
        self.commit_provider_handoff(
            HandoffCheckpoint {
                replacement,
                summary,
                response_id,
                version,
                world_state,
            },
            step,
            cancellation,
        )
        .await?;
        self.recompute_token_usage(target).await;
        Ok(())
    }

    async fn commit_provider_handoff(
        &self,
        checkpoint: HandoffCheckpoint,
        step: &StepContext,
        cancellation: &CancellationToken,
    ) -> Result<()> {
        let HandoffCheckpoint {
            mut replacement,
            summary,
            response_id,
            version,
            world_state,
        } = checkpoint;
        for item in &mut replacement {
            Self::assign_missing_response_item_id(&mut item.item);
        }
        let _guard = super::thread_settings::acquire_persistence_lock(self).await;
        let (checkpoint, settings, number, next_ids) = {
            let state = self.state.lock().await;
            if cancellation.is_cancelled() {
                return Err(CodexErr::TurnAborted);
            }
            if state.history.history_version() != version {
                return Err(CodexErr::InvalidRequest(
                    "History changed while preparing handoff; retry the turn".into(),
                ));
            }
            let ids = state.auto_compact_window_ids();
            let number = state.auto_compact_window_number().saturating_add(1);
            let next_ids = crate::state::AutoCompactWindowIds {
                first_window_id: ids.first_window_id,
                previous_window_id: Some(ids.window_id),
                window_id: uuid::Uuid::now_v7(),
            };
            let checkpoint = CompactedItem {
                message: summary,
                replacement_history: Some(replacement.clone()),
                retained_context: Some(state.history.retained_context().clone()),
                guardian_history: state.history.guardian_history_checkpoint(),
                mcp_resource_origins: self.services.mcp_runtime.resource_origin_checkpoint(),
                window_number: Some(number),
                first_window_id: Some(next_ids.first_window_id.to_string()),
                previous_window_id: Some(ids.window_id.to_string()),
                window_id: Some(next_ids.window_id.to_string()),
                compaction_response_id: Some(response_id),
                latest_token_usage_record: state.latest_token_usage_record.clone(),
            };
            let mut settings = state
                .session_configuration
                .thread_settings_snapshot(&self.services.turn_environments.selections());
            settings.active_model = state.active_model_source.as_ref().map(|source| {
                codex_protocol::protocol::ProviderModelSelection {
                    model_provider: source.provider_id.clone(),
                    model: source.model.clone(),
                }
            });
            (checkpoint, settings, number, next_ids)
        };
        if let Some(live) = self.live_thread() {
            live.append_items(&[
                RolloutItem::Compacted(checkpoint),
                RolloutItem::WorldState(codex_protocol::protocol::WorldStateItem::full(
                    world_state.snapshot().into_object(),
                )),
                RolloutItem::TurnContext(step.to_turn_context_item()),
                RolloutItem::EventMsg(codex_protocol::protocol::EventMsg::ThreadSettingsApplied(
                    codex_protocol::protocol::ThreadSettingsAppliedEvent {
                        thread_id: Some(self.thread_id()),
                        thread_settings: settings,
                    },
                )),
            ])
            .await
            .map_err(|error| CodexErr::InvalidRequest(error.to_string()))?;
            live.flush()
                .await
                .map_err(|error| CodexErr::InvalidRequest(error.to_string()))?;
        }
        let mut state = self.state.lock().await;
        state.replace_annotated_history(
            replacement,
            Some(step.to_turn_context_item()),
            HistoryReplacement::Compaction,
        );
        state
            .history
            .set_world_state_baseline(world_state.snapshot());
        state.restore_auto_compact_window(number, next_ids);
        state.reasoning_effort_pin = crate::state::ReasoningEffortPin::Compacted;
        state.queue_pending_session_start_source(codex_hooks::SessionStartSource::Compact);
        Ok(())
    }
}

fn request_tokens(
    prompt: &Prompt,
    step: &StepContext,
    metadata: &CodexResponsesMetadata,
) -> Result<i64> {
    let request = step.turn.model_runtime.client.build_responses_request(
        prompt,
        &step.settings.model_info,
        step.settings.reasoning_effort().cloned(),
        step.settings.reasoning_summary,
        step.settings.service_tier.clone(),
        metadata,
    )?;
    let input = request
        .input
        .iter()
        .map(estimate_item_token_count)
        .fold(0i64, i64::saturating_add);
    let extras = serde_json::to_string(&(&request.tools, &request.text))
        .map_err(|error| CodexErr::InvalidRequest(error.to_string()))?;
    Ok(input
        .saturating_add(approx_token_count(&request.instructions) as i64)
        .saturating_add(approx_token_count(&extras) as i64))
}

struct HandoffCheckpoint {
    replacement: Vec<ResponseItemEnvelope>,
    summary: String,
    response_id: String,
    version: u64,
    world_state: std::sync::Arc<crate::context::world_state::WorldState>,
}
