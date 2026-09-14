use super::*;
use codex_history::CodexHarnessMetadata;
use pretty_assertions::assert_eq;
use serde_json::json;

fn envelope(value: serde_json::Value, source: Option<ModelOutputSource>) -> ResponseItemEnvelope {
    ResponseItemEnvelope {
        item: serde_json::from_value(value).unwrap(),
        metadata: Some(CodexHarnessMetadata {
            model_source: source,
            ..Default::default()
        }),
    }
}

#[test]
fn projects_tool_pairs_and_drops_foreign_reasoning_without_mutating_history() {
    let source = ModelOutputSource {
        provider_id: "a".into(),
        model: "same/id".into(),
        identity: "account-a".into(),
    };
    let target = ModelOutputSource {
        provider_id: "b".into(),
        model: "same/id".into(),
        identity: "account-b".into(),
    };
    let history = vec![
        envelope(
            json!({"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":"private reasoning"}],"encrypted_content":"opaque"}),
            Some(source.clone()),
        ),
        envelope(
            json!({"type":"custom_tool_call","id":"ctc-a","call_id":"duplicate","name":"apply_patch","input":"patch"}),
            Some(source.clone()),
        ),
        envelope(
            json!({"type":"custom_tool_call_output","call_id":"duplicate","output":"file changed"}),
            Some(source),
        ),
        envelope(
            json!({"type":"function_call","call_id":"duplicate","name":"read","arguments":"{}"}),
            Some(target.clone()),
        ),
        envelope(
            json!({"type":"function_call_output","call_id":"duplicate","output":"new contents"}),
            Some(target.clone()),
        ),
    ];
    let original = history.clone();
    let projected = project(
        history.clone(),
        &target,
        ProviderCapabilities::default(),
        None,
    )
    .unwrap();
    assert_eq!(history, original);
    let values: Vec<_> = projected
        .into_iter()
        .map(|item| serde_json::to_value(item).unwrap())
        .collect();
    assert_eq!(
        values,
        vec![
            json!({"type":"custom_tool_call","call_id":"duplicate","name":"apply_patch","input":"patch"}),
            json!({"type":"custom_tool_call_output","call_id":"duplicate","output":"file changed"}),
            json!({"type":"function_call","call_id":"codex_history_3","name":"read","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"codex_history_3","output":"new contents"}),
        ]
    );
}

#[test]
fn opaque_state_needs_matching_provider_account_model_and_compaction_compatibility() {
    let source = ModelOutputSource {
        provider_id: "a".into(),
        model: "m".into(),
        identity: "key-a".into(),
    };
    let item = envelope(
        json!({"type":"compaction","encrypted_content":"never system text"}),
        Some(source.clone()),
    );
    assert_eq!(
        project(
            vec![item.clone()],
            &source,
            ProviderCapabilities::default(),
            None
        )
        .unwrap(),
        vec![item.item.clone()]
    );
    for target in [
        ModelOutputSource {
            provider_id: "b".into(),
            ..source.clone()
        },
        ModelOutputSource {
            model: "other".into(),
            ..source.clone()
        },
        ModelOutputSource {
            identity: "key-b".into(),
            ..source.clone()
        },
    ] {
        assert!(
            project(
                vec![item.clone()],
                &target,
                ProviderCapabilities::default(),
                None
            )
            .is_err()
        );
    }
    let mut compatible = item.clone();
    compatible.metadata.as_mut().unwrap().compaction_model_hash = Some("shared".into());
    let target = ModelOutputSource {
        model: "other".into(),
        ..source.clone()
    };
    assert_eq!(
        project(
            vec![compatible.clone()],
            &target,
            ProviderCapabilities::default(),
            Some("shared")
        )
        .unwrap(),
        vec![item.item.clone()]
    );
    let different_owner = ModelOutputSource {
        identity: "other-owner".into(),
        ..target
    };
    assert!(
        project(
            vec![compatible],
            &different_owner,
            ProviderCapabilities::default(),
            Some("shared")
        )
        .is_err()
    );
    let unknown = ResponseItemEnvelope::new(item.item);
    assert!(
        project(
            vec![unknown],
            &source,
            ProviderCapabilities::default(),
            None
        )
        .is_err()
    );
}

#[test]
fn readable_agent_message_crosses_providers_without_summarizing_or_rewriting_history() {
    let source = ModelOutputSource {
        provider_id: "a".into(),
        model: "m".into(),
        identity: "owner".into(),
    };
    let target = ModelOutputSource {
        provider_id: "b".into(),
        ..source.clone()
    };
    let item = codex_protocol::protocol::InterAgentCommunication::new(
        codex_protocol::AgentPath::root().join("worker").unwrap(),
        codex_protocol::AgentPath::root(),
        Vec::new(),
        "The patch is ready".into(),
        false,
    )
    .to_model_input_item();
    let original = ResponseItemEnvelope {
        item: item.clone(),
        metadata: Some(CodexHarnessMetadata {
            model_source: Some(source),
            ..Default::default()
        }),
    };
    let projected = project(
        vec![original.clone()],
        &target,
        ProviderCapabilities {
            namespace_tools: false,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(
        projected,
        vec![ResponseItem::Message {
            id: None,
            role: "user".into(),
            content: vec![
                ContentItem::InputText {
                    text: "Agent message from /root/worker to /root:\n".into()
                },
                ContentItem::InputText {
                    text: "The patch is ready".into()
                }
            ],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }]
    );
    assert_eq!(original.item, item);
}
