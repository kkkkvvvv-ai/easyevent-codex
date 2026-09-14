use super::*;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::ThreadRolledBackEvent;
use codex_protocol::protocol::TurnContextItem;
use pretty_assertions::assert_eq;
use serde_json::json;

fn turn(id: &str, provider: &str) -> Vec<RolloutItem> {
    let context: TurnContextItem = serde_json::from_value(json!({
        "turn_id": id, "cwd": std::env::current_dir().unwrap(),
        "approval_policy": "never", "sandbox_policy": {"type":"read-only"},
        "model": "organization/raw-model", "summary":"auto",
        "model_source":{"provider_id":provider,"model":"organization/raw-model","identity":"owner"}
    }))
    .unwrap();
    vec![
        RolloutItem::EventMsg(EventMsg::TurnStarted(
            serde_json::from_value(json!({"turn_id":id})).unwrap(),
        )),
        RolloutItem::TurnContext(context),
        RolloutItem::ResponseItem(
            ResponseItem::Message {
                id: None,
                role: "user".into(),
                content: vec![ContentItem::InputText { text: id.into() }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            }
            .into(),
        ),
    ]
}

#[test]
fn rollback_discards_deleted_model_settings_and_keeps_later_selection() {
    let first = turn("first", "alpha");
    let mut history = first.clone();
    history.extend(turn("second", "beta"));
    history.push(RolloutItem::EventMsg(EventMsg::ThreadRolledBack(
        ThreadRolledBackEvent { num_turns: 1 },
    )));
    assert_eq!(
        latest_persisted_model_selection(&history),
        latest_persisted_model_selection(&first)
    );
    let third = turn("third", "gamma");
    history.extend(third.clone());
    assert_eq!(
        latest_persisted_model_selection(&history),
        latest_persisted_model_selection(&third)
    );
    history.push(RolloutItem::EventMsg(EventMsg::ThreadRolledBack(
        ThreadRolledBackEvent { num_turns: 1 },
    )));
    assert_eq!(
        latest_persisted_model_selection(&history),
        Some(PersistedModelSelection {
            model_provider: "alpha".into(),
            model: "organization/raw-model".into(),
            reasoning_effort: None,
        })
    );
}

#[test]
fn context_only_segments_do_not_consume_rollback_budget() {
    let first = turn("first", "alpha");
    let mut history = first.clone();
    history.extend(turn("second", "beta"));
    history.extend(turn("context-only", "gamma").into_iter().take(2));
    history.push(RolloutItem::EventMsg(EventMsg::ThreadRolledBack(
        ThreadRolledBackEvent { num_turns: 1 },
    )));
    assert_eq!(
        latest_persisted_model_selection(&history),
        latest_persisted_model_selection(&first)
    );
    history.push(RolloutItem::EventMsg(EventMsg::ThreadRolledBack(
        ThreadRolledBackEvent {
            num_turns: u32::MAX,
        },
    )));
    assert_eq!(latest_persisted_model_selection(&history), None);
}

#[test]
fn rollback_distinguishes_in_turn_updates_from_later_standalone_choices() {
    let completed = |id: &str| {
        RolloutItem::EventMsg(EventMsg::TurnComplete(
            serde_json::from_value(json!({"turn_id":id,"last_agent_message":null})).unwrap(),
        ))
    };
    let settings = RolloutItem::EventMsg(EventMsg::ThreadSettingsApplied(
        codex_protocol::protocol::ThreadSettingsAppliedEvent {
            thread_id: None,
            thread_settings: serde_json::from_value(json!({
                "model":"next-model", "model_provider_id":"gamma",
                "cwd":std::env::current_dir().unwrap(), "approval_policy":"never", "approvals_reviewer":"user",
                "permission_profile":codex_protocol::models::PermissionProfile::read_only(),
                "collaboration_mode":{"mode":"default","settings":{"model":"next-model","reasoning_effort":"high","developer_instructions":null}}
            })).unwrap(),
        },
    ));
    let rollback = RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
        num_turns: 1,
    }));
    let mut first = turn("first", "alpha");
    first.push(completed("first"));
    let mut during = first.clone();
    during.extend(turn("second", "beta"));
    during.extend([settings.clone(), completed("second"), rollback.clone()]);
    assert_eq!(
        latest_persisted_model_selection(&during),
        latest_persisted_model_selection(&first)
    );
    let mut after = first;
    after.extend(turn("second", "beta"));
    after.extend([completed("second"), settings, rollback]);
    assert_eq!(
        latest_persisted_model_selection(&after),
        Some(PersistedModelSelection {
            model_provider: "gamma".into(),
            model: "next-model".into(),
            reasoning_effort: Some(ReasoningEffort::High),
        })
    );
}
