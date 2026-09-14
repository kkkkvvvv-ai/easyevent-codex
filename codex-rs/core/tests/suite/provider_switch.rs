use anyhow::Context;
use anyhow::Result;
use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_protocol::protocol::ThreadSettingsOverrides;
use core_test_support::responses;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use test_case::test_case;

#[test_case("deepseek", "deepseek-flash", Some("high"), Some("custom"); "deepseek")]
#[test_case("glm-cn", "glm-5.3", Some("high"), Some("custom"); "glm")]
#[test_case("glm-cn", "glm-5-turbo", None, Some("custom"); "glm_turbo")]
#[test_case("kimi", "kimi-for-coding", Some("high"), None; "kimi")]
#[test_case("hy-cn", "hy4-preview", None, None; "hy")]
#[test_case("minimax-cn", "MiniMax-M3", Some("high"), None; "minimax")]
#[test_case("openrouter", "~openai/gpt-latest", None, None; "openrouter")]
#[tokio::test]
async fn provider_model_metadata_controls_responses_requests(
    provider: &str,
    model: &str,
    effort: Option<&str>,
    patch_type: Option<&str>,
) -> Result<()> {
    let server = responses::start_mock_server().await;
    let catalog = codex_model_provider::recommended_responses_models(
        codex_model_provider_info::responses_provider_preset(provider)
            .context("known hosted provider")?,
    );
    let test = test_codex()
        .with_model(model)
        .with_config(move |config| {
            config.model_provider.name = "Hosted Responses mock".into();
            config.model_provider.requires_openai_auth = false;
            config.model_provider.experimental_bearer_token = Some("hosted-key".into());
            config.model_catalog = Some(catalog);
            config.model_reasoning_effort = None;
        })
        .build_with_auto_env(&server)
        .await?;
    let response = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_assistant_message("answer", "OK"),
            responses::ev_completed("done"),
        ]),
    )
    .await;
    test.submit_text_turn("Hello").await?;
    let request = response.single_request();
    let body = request.body_json();
    assert_eq!(
        (
            body["model"].as_str(),
            body["reasoning"]["effort"].as_str(),
            body["reasoning"]["summary"].as_str()
        ),
        (Some(model), effort, None)
    );
    let patch = body["tools"]
        .as_array()
        .context("Responses tool catalog")?
        .iter()
        .find(|tool| tool["name"] == "apply_patch");
    assert_eq!(patch.and_then(|tool| tool["type"].as_str()), patch_type);
    assert_eq!(
        request.header("authorization"),
        Some("Bearer hosted-key".into())
    );
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[test_case(ThreadHistoryMode::Legacy; "legacy")]
#[test_case(ThreadHistoryMode::Paginated; "paginated")]
#[tokio::test]
async fn provider_switch_preserves_thread_and_routes_history_with_new_credentials(
    mode: ThreadHistoryMode,
) -> Result<()> {
    let first = responses::start_mock_server().await;
    let second = responses::start_mock_server().await;
    let second_url = format!("{}/v1", second.uri());
    let mut test = test_codex()
        .with_history_mode(mode)
        .with_config(move |config| {
            config.update_plan_enabled = true;
            config.model_provider_id = "provider-a".into();
            config.model_provider.name = "Provider A".into();
            config.model_provider.requires_openai_auth = false;
            config.model_provider.experimental_bearer_token = Some("key-a".into());
            config
                .model_providers
                .insert("provider-a".into(), config.model_provider.clone());
            config.model_providers.insert(
                "provider-b".into(),
                ModelProviderInfo {
                    name: "Provider B".into(),
                    base_url: Some(second_url),
                    experimental_bearer_token: Some("key-b".into()),
                    ..Default::default()
                },
            );
        })
        .build_with_auto_env(&first)
        .await?;
    let thread_id = test.session_configured.thread_id;
    let reasoning = responses::ev_reasoning_item(
        "reason-a",
        &["remembering the user's request"],
        &["private"],
    );
    let encrypted = reasoning["item"]["encrypted_content"]
        .as_str()
        .context("encrypted reasoning fixture")?
        .to_string();
    let before = responses::mount_sse_once(
        &first,
        responses::sse(vec![
            responses::ev_response_created("first"),
            reasoning,
            responses::ev_assistant_message("message-a", "I will remember the blue notebook."),
            responses::ev_completed("first"),
        ]),
    )
    .await;
    test.submit_text_turn("Remember my blue notebook").await?;
    assert_eq!(
        before.single_request().header("authorization"),
        Some("Bearer key-a".into())
    );

    test.codex
        .switch_model_provider(
            "provider-b".into(),
            ThreadSettingsOverrides {
                model: Some("gpt-5.5".into()),
                ..Default::default()
            },
        )
        .await?;
    let after = responses::mount_sse_sequence(
        &second,
        vec![
            responses::sse(vec![
                responses::ev_response_created("tool-turn"),
                responses::ev_function_call(
                    "plan-call",
                    "update_plan",
                    r#"{"plan":[{"step":"Remember the notebook","status":"completed"}]}"#,
                ),
                responses::ev_completed("tool-turn"),
            ]),
            responses::sse_completed("second"),
        ],
    )
    .await;
    test.submit_text_turn("What did I ask you to remember?")
        .await?;
    let requests = after.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].function_call_output_text("plan-call"),
        Some("Plan updated".into())
    );
    let request = &requests[0];
    assert_eq!(request.header("authorization"), Some("Bearer key-b".into()));
    let body = request.body_json().to_string();
    assert!(body.contains("blue notebook"));
    assert!(!body.contains(&encrypted));
    assert!(request.body_json().get("previous_response_id").is_none());
    assert_eq!(
        test.codex
            .thread_settings_snapshot()
            .await
            .model_provider_id,
        "provider-b"
    );
    assert_eq!(test.codex.session_configured().thread_id, thread_id);

    // A restored runtime still excludes the other provider's opaque reasoning.
    let saved_config = test.codex.config().await.as_ref().clone();
    test.codex.flush_rollout().await?;
    let stored = test
        .thread_store
        .load_latest_model_context(codex_thread_store::LoadThreadHistoryParams {
            thread_id,
            include_archived: true,
        })
        .await?;
    test.codex.shutdown_and_wait().await?;
    let resumed = test
        .thread_manager
        .resume_thread_with_history(
            saved_config,
            codex_history::InitialHistory::Resumed(codex_history::ResumedHistory {
                conversation_id: thread_id,
                history: std::sync::Arc::new(stored.items),
                rollout_path: test.session_configured.rollout_path.clone(),
            }),
            test.thread_manager.auth_manager(),
            /*parent_trace*/ None,
            Default::default(),
        )
        .await?;
    test.codex = resumed.thread;
    let resumed_response =
        responses::mount_sse_once(&second, responses::sse_completed("resumed")).await;
    test.submit_text_turn("Remember this after a restart")
        .await?;
    assert_eq!(
        resumed_response.single_request().header("authorization"),
        Some("Bearer key-b".into())
    );
    assert!(
        !resumed_response
            .single_request()
            .body_json()
            .to_string()
            .contains(&encrypted)
    );

    test.codex
        .switch_model_provider(
            "provider-a".into(),
            ThreadSettingsOverrides {
                model: Some("gpt-5.5".into()),
                ..Default::default()
            },
        )
        .await?;
    let back = responses::mount_sse_once(&first, responses::sse_completed("third")).await;
    test.submit_text_turn("Continue after switching back")
        .await?;
    assert_eq!(
        back.single_request().header("authorization"),
        Some("Bearer key-a".into())
    );
    assert!(
        back.single_request()
            .body_json()
            .to_string()
            .contains("blue notebook")
    );
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[tokio::test]
async fn provider_switch_materializes_encrypted_compaction_before_changing_endpoint() -> Result<()>
{
    let first = responses::start_mock_server().await;
    let second = responses::start_mock_server().await;
    let second_url = format!("{}/v1", second.uri());
    let test = test_codex()
        .with_config(move |config| {
            config.model_providers.insert(
                "provider-b".into(),
                ModelProviderInfo {
                    base_url: Some(second_url),
                    ..Default::default()
                },
            );
        })
        .build_with_auto_env(&first)
        .await?;
    test.codex
        .inject_response_items(vec![codex_protocol::models::ResponseItem::Compaction {
            id: None,
            encrypted_content: "endpoint-owned-context".into(),
            internal_chat_message_metadata_passthrough: None,
        }])
        .await?;
    let summary = responses::mount_sse_once(
        &first,
        responses::sse(vec![
            responses::ev_response_created("summary"),
            responses::ev_assistant_message(
                "portable",
                "The user wants to keep the blue notebook.",
            ),
            responses::ev_completed("summary"),
        ]),
    )
    .await;
    tokio::time::timeout(
        std::time::Duration::from_secs(/*secs*/ 20),
        test.codex.switch_model_provider(
            "provider-b".into(),
            ThreadSettingsOverrides {
                model: Some("gpt-5.5".into()),
                ..Default::default()
            },
        ),
    )
    .await??;
    assert!(
        summary
            .single_request()
            .body_json()
            .to_string()
            .contains("endpoint-owned-context")
    );
    let next = responses::mount_sse_once(&second, responses::sse_completed("next")).await;
    test.submit_text_turn("Continue").await?;
    let body = next.single_request().body_json().to_string();
    assert!(body.contains("blue notebook"));
    assert!(!body.contains("endpoint-owned-context"));
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[tokio::test]
async fn provider_switch_rejects_running_turn_without_mutating_settings() -> Result<()> {
    use core_test_support::streaming_sse::StreamingSseChunk;
    use core_test_support::streaming_sse::start_streaming_sse_server;
    let (release, gate) = tokio::sync::oneshot::channel();
    let (server, _) = start_streaming_sse_server(vec![vec![StreamingSseChunk {
        gate: Some(gate),
        body: responses::sse_completed("busy-turn"),
    }]])
    .await;
    let test = test_codex().build_with_streaming_server(&server).await?;
    let initial = test.codex.thread_settings_snapshot().await;
    test.codex
        .start_turn_if_idle(codex_core::TurnInputRequest::user_input(vec![
            codex_protocol::user_input::UserInput::Text {
                text: "keep working".into(),
                text_elements: Vec::new(),
            },
        ]))
        .await?;
    let result = test
        .codex
        .switch_model_provider(
            "deepseek".into(),
            ThreadSettingsOverrides {
                model: Some("deepseek-flash".into()),
                ..Default::default()
            },
        )
        .await;
    assert!(result.is_err());
    assert_eq!(test.codex.thread_settings_snapshot().await, initial);
    let _ = release.send(());
    core_test_support::wait_for_event(&test.codex, |event| {
        matches!(event, codex_protocol::protocol::EventMsg::TurnComplete(_))
    })
    .await;
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[tokio::test]
async fn provider_reselection_does_not_require_old_credentials_to_summarize_portable_tool_history()
-> Result<()> {
    let server = responses::start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.model_provider_id = "deepseek".into();
            config
                .model_providers
                .insert("deepseek".into(), config.model_provider.clone());
        })
        .build_with_auto_env(&server)
        .await?;
    test.codex.inject_response_items(vec![
        serde_json::from_value(serde_json::json!({"type":"custom_tool_call", "call_id":"prior-patch", "name":"apply_patch", "input":"*** Begin Patch\n*** End Patch"}))?,
        serde_json::from_value(serde_json::json!({"type":"custom_tool_call_output", "call_id":"prior-patch", "output":"No files changed"}))?,
    ]).await?;
    tokio::time::timeout(
        std::time::Duration::from_secs(/*secs*/ 5),
        test.codex.switch_model_provider(
            "deepseek".into(),
            ThreadSettingsOverrides {
                model: Some("gpt-5.5".into()),
                ..Default::default()
            },
        ),
    )
    .await??;
    let next = responses::mount_sse_once(&server, responses::sse_completed("next")).await;
    test.submit_text_turn("Continue with the updated provider selection")
        .await?;
    assert!(
        next.single_request()
            .body_json()
            .to_string()
            .contains("No files changed")
    );
    test.codex.shutdown_and_wait().await?;
    Ok(())
}
