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
        {"id":"vendor/image", "supported_parameters":[], "architecture":{"output_modalities":["image"]}}
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
    assert_eq!(catalog, expected);
}
