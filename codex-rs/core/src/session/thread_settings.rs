//! Handles persistent thread-settings updates and serializes their persistence
//! with checkpoints written directly to storage.

use super::session::Session;
use super::session::SessionSettingsUpdate;
use super::step_settings::StepSettingsUpdate;
use crate::config::ConstraintResult;
use codex_history::RolloutItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::ErrorEvent;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ThreadSettingsAppliedEvent;
use codex_protocol::protocol::ThreadSettingsOverrides;
use codex_protocol::protocol::ThreadSettingsSnapshot;
use codex_thread_store::ThreadStoreResult;
use std::sync::Arc;
use tokio::sync::SemaphorePermit;

impl Session {
    #[expect(
        clippy::await_holding_invalid_type,
        reason = "idle turn validation and reservation must remain atomic"
    )]
    pub(crate) async fn switch_model_provider(
        self: &Arc<Self>,
        provider_id: String,
        mut overrides: ThreadSettingsOverrides,
    ) -> codex_protocol::error::Result<()> {
        let hosted_target =
            codex_model_provider_info::responses_provider_preset(&provider_id).is_some();
        overrides.effort = Some(overrides.effort.flatten());
        overrides.service_tier = Some(None);
        let mut updates = prepare_update(overrides);
        updates.model_provider = Some(provider_id);
        // Remote compaction state belongs to its original endpoint. Materialize a
        // portable summary there before publishing the new provider runtime.
        let mut idle = self.active_turn.lock().await;
        if idle.is_some() {
            return Err(codex_protocol::error::CodexErr::InvalidRequest(
                "Wait for the current turn to finish before switching providers".into(),
            ));
        }
        let changing_provider = {
            let state = self.state.lock().await;
            let candidate = self
                .apply_session_settings(&state.session_configuration, &updates)
                .map_err(|error| {
                    codex_protocol::error::CodexErr::InvalidRequest(error.to_string())
                })?;
            state
                .session_configuration
                .original_config_do_not_use
                .model_provider_id
                != candidate.original_config_do_not_use.model_provider_id
                || state.session_configuration.provider.info() != candidate.provider.info()
        };
        let needs_summary = self.clone_history().await.raw_items().any(|item| {
            matches!(
                item,
                ResponseItem::Compaction { .. }
                    | ResponseItem::ContextCompaction {
                        encrypted_content: Some(_),
                        ..
                    }
            ) || changing_provider
                && hosted_target
                && matches!(
                    item,
                    ResponseItem::CustomToolCall { .. }
                        | ResponseItem::CustomToolCallOutput { .. }
                        | ResponseItem::FunctionCall {
                            namespace: Some(_),
                            ..
                        }
                        | ResponseItem::FunctionCallOutput {
                            namespace: Some(_),
                            ..
                        }
                        | ResponseItem::ToolSearchCall { .. }
                        | ResponseItem::ToolSearchOutput { .. }
                        | ResponseItem::WebSearchCall { .. }
                        | ResponseItem::ImageGenerationCall { .. }
                        | ResponseItem::LocalShellCall { .. }
                        | ResponseItem::AdditionalTools { .. }
                        | ResponseItem::ConfigurationUpdate { .. }
                        | ResponseItem::CompactionTrigger { .. }
                )
        });
        let reservation = needs_summary.then(|| {
            let reserved = crate::state::ActiveTurn::default();
            let identity = reserved.turn_state.clone();
            *idle = Some(reserved);
            identity
        });
        drop(idle);
        if needs_summary {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(/*secs*/ 60),
                crate::compact::run_inline_auto_compact_task(
                    self.clone(),
                    self.new_default_turn().await,
                    crate::compact::InitialContextInjection::DoNotInject,
                    codex_analytics::CompactionReason::UserRequested,
                    codex_analytics::CompactionPhase::PreTurn,
                ),
            )
            .await;
            let mut active = self.active_turn.lock().await;
            if active
                .as_ref()
                .zip(reservation.as_ref())
                .is_some_and(|(active, identity)| {
                    active.task.is_none() && Arc::ptr_eq(&active.turn_state, identity)
                })
            {
                *active = None;
            } else {
                return Err(codex_protocol::error::CodexErr::Interrupted);
            }
            drop(active);
            result.map_err(|_| {
                codex_protocol::error::CodexErr::InvalidRequest(
                    "Unable to create a portable summary before switching providers".into(),
                )
            })??;
        }
        apply_update(self, super::new_submission_id(), updates)
            .await
            .map_err(|error| codex_protocol::error::CodexErr::InvalidRequest(error.to_string()))?;
        self.checkpoint_thread_settings().await.map_err(|error| {
            codex_protocol::error::CodexErr::InvalidRequest(format!(
                "Provider switched but settings could not be persisted: {error}"
            ))
        })
    }

    /// Captures and flushes current settings under the shared persistence permit.
    pub(crate) async fn checkpoint_thread_settings(&self) -> ThreadStoreResult<()> {
        let _settings_guard = acquire_persistence_lock(self).await;
        if let Some(live_thread) = self.live_thread() {
            live_thread
                .append_items(&[RolloutItem::EventMsg(applied_event(self).await)])
                .await?;
            live_thread.flush().await?;
        }
        Ok(())
    }
}

/// Applies standalone thread settings and reports invalid overrides through the
/// normal event stream.
pub(super) async fn update(
    session: &Arc<Session>,
    submission_id: String,
    overrides: ThreadSettingsOverrides,
) {
    let updates = prepare_update(overrides);
    if let Err(error) = apply_update(session, submission_id.clone(), updates).await {
        session
            .send_event_raw(Event {
                id: submission_id,
                msg: EventMsg::Error(ErrorEvent {
                    misalignment: None,
                    message: format!("invalid thread settings override: {error}"),
                    codex_error_info: Some(CodexErrorInfo::BadRequest),
                }),
            })
            .await;
    }
}

/// Converts protocol overrides into the internal settings update shape.
pub(super) fn prepare_update(overrides: ThreadSettingsOverrides) -> SessionSettingsUpdate {
    let ThreadSettingsOverrides {
        environments,
        runtime_workspace_roots,
        profile_workspace_roots,
        approval_policy,
        approvals_reviewer,
        sandbox_policy,
        permission_profile,
        active_permission_profile,
        windows_sandbox_level,
        model,
        effort,
        summary,
        service_tier,
        collaboration_mode,
        personality,
        disabled_plugin_ids,
    } = overrides;
    SessionSettingsUpdate {
        step_settings: StepSettingsUpdate {
            model,
            effort,
            collaboration_mode,
            reasoning_summary: summary,
            service_tier,
            personality,
            approval_policy,
            approvals_reviewer,
        },
        environments,
        runtime_workspace_roots,
        profile_workspace_roots,
        sandbox_policy,
        permission_profile,
        active_permission_profile,
        windows_sandbox_level,
        disabled_plugin_ids,
        ..Default::default()
    }
}

/// Acquires the shared permit before capturing or changing persistent settings.
pub(super) async fn acquire_persistence_lock(session: &Session) -> SemaphorePermit<'_> {
    session
        .thread_settings_persistence
        .acquire()
        .await
        .unwrap_or_else(|_| unreachable!("thread settings persistence semaphore is never closed"))
}

/// Applies persistent settings and emits the resulting thread-owned snapshot.
pub(super) async fn apply_update(
    session: &Session,
    submission_id: String,
    updates: SessionSettingsUpdate,
) -> ConstraintResult<()> {
    let _settings_guard = acquire_persistence_lock(session).await;
    let commit = session.update_settings(updates).await?;
    emit_applied(session, submission_id, commit.snapshot).await;
    Ok(())
}

/// Emits the snapshot published by one successful settings update.
pub(super) async fn emit_applied(
    session: &Session,
    submission_id: String,
    snapshot: ThreadSettingsSnapshot,
) {
    let msg = EventMsg::ThreadSettingsApplied(ThreadSettingsAppliedEvent {
        thread_id: Some(session.thread_id()),
        thread_settings: snapshot,
    });
    session
        .send_event_raw_without_materializing_rollout(Event {
            id: submission_id,
            msg,
        })
        .await;
}

/// Builds a current thread-owned snapshot for storage checkpoints.
pub(super) async fn applied_event(session: &Session) -> EventMsg {
    EventMsg::ThreadSettingsApplied(ThreadSettingsAppliedEvent {
        thread_id: Some(session.thread_id()),
        thread_settings: session.thread_settings_snapshot().await,
    })
}
