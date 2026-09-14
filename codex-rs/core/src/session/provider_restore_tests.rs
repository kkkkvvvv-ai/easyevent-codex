use super::*;
use codex_history::CodexHarnessMetadata;
use codex_history::RolloutItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ProviderModelSelection;
use codex_protocol::protocol::ThreadSettingsAppliedEvent;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn rollback_restores_surviving_producer_when_saved_active_model_has_no_history() {
    let (session, turn) = crate::session::tests::make_session_and_context().await;
    let source = turn
        .model_runtime
        .source_for(&turn.model_info().slug)
        .unwrap();
    let history = vec![ResponseItemEnvelope {
        item: serde_json::from_value(serde_json::json!({
            "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Surviving plan"}]
        })).unwrap(),
        metadata: Some(CodexHarnessMetadata { model_source: Some(source.clone()), ..Default::default() }),
    }];
    let mut settings = session.thread_settings_snapshot().await;
    settings.active_model = Some(ProviderModelSelection {
        model_provider: source.provider_id.clone(),
        model: "removed-model".into(),
    });
    let saved = [RolloutItem::EventMsg(EventMsg::ThreadSettingsApplied(
        ThreadSettingsAppliedEvent {
            thread_id: Some(session.thread_id()),
            thread_settings: settings,
        },
    ))];
    session
        .restore_model_runtime(&history, /*context*/ None, &saved)
        .await;
    let state = session.state.lock().await;
    assert_eq!(state.active_model_source, Some(source));
    assert!(state.active_model_runtime.is_some());
}
