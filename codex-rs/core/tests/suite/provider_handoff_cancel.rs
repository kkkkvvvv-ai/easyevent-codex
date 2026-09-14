use anyhow::Result;
use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ThreadSettingsOverrides;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::streaming_sse::StreamingSseChunk;
use core_test_support::streaming_sse::start_streaming_sse_server;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn cancellation_keeps_the_original_active_model_and_never_calls_target() -> Result<()> {
    let target = responses::start_mock_server().await;
    let target_url = format!("{}/v1", target.uri());
    let (release, gate) = tokio::sync::oneshot::channel();
    let initial = responses::sse(vec![
        responses::ev_assistant_message("plan", "Review found a missing bounds check"),
        serde_json::json!({"type":"response.output_item.done", "item":{"type":"compaction","encrypted_content":"original-checkpoint"}}),
        responses::ev_completed("initial"),
    ]);
    let (source, _) = start_streaming_sse_server(vec![
        vec![StreamingSseChunk {
            gate: None,
            body: initial,
        }],
        vec![StreamingSseChunk {
            gate: Some(gate),
            body: responses::sse_completed("summary"),
        }],
    ])
    .await;
    let test = test_codex()
        .with_config(move |config| {
            config.model_providers.insert(
                "target".into(),
                ModelProviderInfo {
                    base_url: Some(target_url),
                    ..Default::default()
                },
            );
        })
        .build_with_streaming_server(&source)
        .await?;
    test.submit_text_turn("Review the bounds check").await?;
    let active = test.codex.thread_settings_snapshot().await.active_model;
    test.codex
        .switch_model_provider(
            "target".into(),
            ThreadSettingsOverrides {
                model: Some("gpt-5.5".into()),
                ..Default::default()
            },
        )
        .await?;
    let target_response =
        responses::mount_sse_once(&target, responses::sse_completed("target")).await;
    test.codex
        .start_turn_if_idle(codex_core::TurnInputRequest::user_input(vec![
            UserInput::Text {
                text: "Apply the review fix".into(),
                text_elements: Vec::new(),
            },
        ]))
        .await?;
    source.wait_for_request_count(2).await;
    test.codex.submit(Op::Interrupt).await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnAborted(_))
    })
    .await;
    assert_eq!(
        test.codex.thread_settings_snapshot().await.active_model,
        active
    );
    assert_eq!(
        test.codex
            .thread_settings_snapshot()
            .await
            .model_provider_id,
        "target"
    );
    assert!(target_response.requests().is_empty());
    let _ = release.send(());
    test.codex.shutdown_and_wait().await?;
    source.shutdown().await;
    Ok(())
}
