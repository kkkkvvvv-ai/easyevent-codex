use super::*;
use codex_http_client::OutboundProxyPolicy;
use codex_model_provider_info::responses_provider_preset;
use pretty_assertions::assert_eq;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

#[tokio::test]
async fn validation_uses_responses_sse_and_a_synthetic_prompt() {
    let server = MockServer::start().await;
    let preset = ResponsesProviderPreset {
        base_url: Box::leak(server.uri().into_boxed_str()),
        ..*responses_provider_preset("deepseek").unwrap()
    };
    Mock::given(method("POST")).and(path("/responses")).and(header("authorization", "Bearer secret-test-key"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"test\",\"status\":\"completed\",\"output\":[]}}\n\n"
        )).mount(&server).await;
    validate_responses_key(
        HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        &preset,
        "secret-test-key",
        "deepseek-flash",
    )
    .await
    .unwrap();
    validate_responses_key(
        HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        &preset,
        "secret-test-key",
        "deepseek-flash",
    )
    .await
    .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[0].body).unwrap(),
        json!({
            "model": "deepseek-flash", "input": "Reply with OK.", "stream": true, "store": false, "max_output_tokens": 1024,
        })
    );
}

#[tokio::test]
async fn validation_redacts_access_quota_and_redirect_failures() {
    for status in [403, 429, 302] {
        let server = MockServer::start().await;
        let other = MockServer::start().await;
        let preset = ResponsesProviderPreset {
            base_url: Box::leak(server.uri().into_boxed_str()),
            ..*responses_provider_preset("hy-intl").unwrap()
        };
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("location", format!("{}/responses", other.uri()))
                    .set_body_string("secret-access-key"),
            )
            .mount(&server)
            .await;
        let error = validate_responses_key(
            HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
            &preset,
            "secret-access-key",
            "hy4-preview",
        )
        .await
        .unwrap_err();
        assert!(!error.contains("secret-access-key"));
        assert!(other.received_requests().await.unwrap().is_empty());
    }
}

#[test]
fn model_catalog_cache_isolates_endpoint_and_credential_identity() {
    let home = tempfile::tempdir().unwrap();
    let store = codex_login::auth::ProviderCredentialStore::new(
        home.path().to_path_buf(),
        codex_login::AuthCredentialsStoreMode::File,
        codex_login::AuthKeyringBackendKind::Direct,
    );
    let preset = ResponsesProviderPreset {
        base_url: "https://catalog-isolation.test",
        ..*responses_provider_preset("deepseek").unwrap()
    };
    let mut catalog = crate::recommended_responses_models(&preset);
    catalog.models.push(hosted_responses_model("private-model"));
    crate::cache_responses_models(&store, &preset, "key-one", catalog.clone()).unwrap();
    assert_eq!(
        crate::cached_responses_models(&store, &preset, "key-one"),
        catalog
    );
    assert_eq!(
        crate::cached_responses_models(&store, &preset, "key-two"),
        crate::recommended_responses_models(&preset)
    );
    let other = ResponsesProviderPreset {
        base_url: "https://different-endpoint.test",
        ..preset
    };
    assert_eq!(
        crate::cached_responses_models(&store, &other, "key-one"),
        crate::recommended_responses_models(&other)
    );
}

#[tokio::test]
async fn validation_times_out_without_echoing_credentials() {
    let server = MockServer::start().await;
    let preset = ResponsesProviderPreset {
        base_url: Box::leak(server.uri().into_boxed_str()),
        ..*responses_provider_preset("deepseek").unwrap()
    };
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(/*secs*/ 46)))
        .mount(&server)
        .await;
    let error = validate_responses_key(
        HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        &preset,
        "secret-timeout-key",
        "deepseek-flash",
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        "Provider validation timed out; the previous configuration was preserved"
    );
}

#[tokio::test]
async fn unlisted_models_must_complete_a_synthetic_tool_call() {
    let server = MockServer::start().await;
    let preset = ResponsesProviderPreset {
        base_url: Box::leak(server.uri().into_boxed_str()),
        ..*responses_provider_preset("openrouter").unwrap()
    };
    let completed = "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"probe\",\"status\":\"completed\",\"output\":[]}}\n\n";
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(completed),
        )
        .mount(&server)
        .await;
    let factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault);
    let error = validate_responses_key(factory.clone(), &preset, "probe-key", "vendor/unlisted")
        .await
        .unwrap_err();
    assert!(error.contains("tool-call probe"));
    server.reset().await;
    let call = json!({"type": "response.output_item.done", "item": {"type": "function_call", "call_id": "probe-call", "name": "codex_connection_test", "arguments": "{\"value\":\"OK\"}"}});
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("data: {call}\n\n{completed}")),
        )
        .mount(&server)
        .await;
    validate_responses_key(factory, &preset, "probe-key", "vendor/unlisted")
        .await
        .unwrap();
    let request = server.received_requests().await.unwrap().pop().unwrap();
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["tools"][0]["name"], "codex_connection_test");
    assert_eq!(body["input"], "Call codex_connection_test with value OK.");
}

#[tokio::test]
async fn validation_rejects_incomplete_streams_and_redacts_http_bodies() {
    let server = MockServer::start().await;
    let preset = ResponsesProviderPreset {
        base_url: Box::leak(server.uri().into_boxed_str()),
        ..*responses_provider_preset("deepseek").unwrap()
    };
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("secret-test-key"))
        .mount(&server)
        .await;
    let error = validate_responses_key(
        HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        &preset,
        "secret-test-key",
        "deepseek-flash",
    )
    .await
    .unwrap_err();
    assert!(!error.contains("secret-test-key"));
    assert!(error.contains("401"));
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(
                    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"test\"}}\n\n",
                ),
        )
        .mount(&server)
        .await;
    assert!(
        validate_responses_key(
            HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
            &preset,
            "secret-test-key",
            "deepseek-flash"
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn discovery_filters_non_tool_openrouter_models_and_keeps_recommendations() {
    let server = MockServer::start().await;
    let preset = ResponsesProviderPreset {
        base_url: Box::leak(server.uri().into_boxed_str()),
        ..*responses_provider_preset("openrouter").unwrap()
    };
    Mock::given(method("GET")).and(path("/models")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [
        {"id":"vendor/coder", "supported_parameters":["tools"], "architecture":{"output_modalities":["text"],"input_modalities":["text","image"]}, "context_length":65536},
        {"id":"vendor/image", "supported_parameters":[], "architecture":{"output_modalities":["image"]}},
        {"id":"vendor/unknown"}
    ]}))).mount(&server).await;
    let catalog = discover_responses_models(
        HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        &preset,
        "key",
    )
    .await
    .unwrap();
    let mut expected = crate::recommended_responses_models(&preset);
    let mut coder = hosted_responses_model("vendor/coder");
    coder.priority = 2;
    coder.context_window = Some(65536);
    coder.max_context_window = Some(65536);
    coder.input_modalities = vec![InputModality::Text, InputModality::Image];
    expected.models.push(coder);
    let mut unknown = hosted_responses_model("vendor/unknown");
    unknown.priority = 3;
    expected.models.push(unknown);
    assert_eq!(catalog, expected);
}
