//! Provider setup uses the same SSE decoder as normal model requests.
//! Responses and HTTP diagnostics are intentionally not echoed into setup errors.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use codex_api::Compression;
use codex_api::ResponseEvent;
use codex_api::ResponsesClient;
use codex_http_client::ClientRouteClass;
use codex_http_client::HttpClient;
use codex_http_client::HttpClientBuilder;
use codex_http_client::HttpClientFactory;
use codex_http_client::ReqwestTransport;
use codex_model_provider_info::ResponsesProviderPreset;
use codex_models_manager::hosted_responses_model;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelsResponse;
use futures::StreamExt;
use http::HeaderMap;
use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;

use crate::BearerAuthProvider;

type ValidationProofs = Vec<([u8; 32], Instant)>;
static VERIFIED_KEYS: OnceLock<Mutex<ValidationProofs>> = OnceLock::new();

async fn setup_client(factory: HttpClientFactory, url: String) -> Result<HttpClient, String> {
    tokio::task::spawn_blocking(move || {
        HttpClientBuilder::new()
            .without_redirects()
            .without_request_logging()
            .connect_timeout(Duration::from_secs(/*secs*/ 10))
            .build_respecting_outbound_proxy_policy(&factory, &url, ClientRouteClass::Api)
            .map_err(|_| "Unable to create the provider HTTP client".to_string())
    })
    .await
    .map_err(|_| "Unable to initialize provider networking".to_string())?
}

/// Verify a fixed, synthetic prompt. No caller history or tools are sent.
pub async fn validate_responses_key(
    factory: HttpClientFactory,
    preset: &ResponsesProviderPreset,
    key: &str,
    model: &str,
) -> Result<(), String> {
    if key.trim().is_empty() || key.trim().chars().any(char::is_control) {
        return Err("API key must be nonempty and contain no control characters".to_string());
    }
    if model.is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
        return Err("Invalid model ID".to_string());
    }
    let mut identity = Sha256::new();
    for field in [preset.base_url, key.trim(), model] {
        identity.update(field.len().to_le_bytes());
        identity.update(field.as_bytes());
    }
    let identity: [u8; 32] = identity.finalize().into();
    if let Ok(mut verified) = VERIFIED_KEYS.get_or_init(Mutex::default).lock() {
        verified.retain(|(_, at)| at.elapsed() < Duration::from_secs(/*secs*/ 120));
        if verified.iter().any(|(cached, _)| cached == &identity) {
            return Ok(());
        }
    }
    let client = setup_client(factory, format!("{}/responses", preset.base_url)).await?;
    let mut provider = preset
        .provider_info()
        .to_api_provider(/*auth_mode*/ None)
        .map_err(|_| "Invalid provider endpoint".to_string())?;
    provider.retry.max_attempts = 1;
    let client = ResponsesClient::new(
        ReqwestTransport::from_http_client(client),
        provider,
        Arc::new(BearerAuthProvider::new(key.trim().to_string())),
    );
    let probe_tools = !preset.models.contains(&model);
    let mut body = json!({
        "model": model, "input": "Reply with OK.", "stream": true,
        "store": false, "max_output_tokens": 1024,
    });
    if probe_tools {
        body["input"] = json!("Call codex_connection_test with value OK.");
        body["tools"] = json!([{
            "type": "function", "name": "codex_connection_test",
            "description": "A connection test; it has no side effects.",
            "parameters": {"type": "object", "properties": {"value": {"type": "string"}}, "required": ["value"], "additionalProperties": false},
        }]);
        body["tool_choice"] = json!("auto");
    }
    tokio::time::timeout(Duration::from_secs(/*secs*/ 45), async {
        let mut stream = client
            .stream(
                body,
                HeaderMap::new(),
                Compression::None,
                /*turn_state*/ None,
            )
            .await
            .map_err(setup_error)?;
        let mut events = 0usize;
        let mut tool_completed = false;
        while let Some(event) = stream.next().await {
            events += 1;
            if events > 4096 {
                return Err("Provider validation exceeded the response limit".to_string());
            }
            let event = event.map_err(setup_error)?;
            if let ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { name, arguments, .. }) = &event {
                tool_completed |= name == "codex_connection_test" && serde_json::from_str::<Value>(arguments).is_ok_and(|args| args == json!({"value": "OK"}));
            }
            if matches!(event, ResponseEvent::Completed { .. }) {
                if probe_tools && !tool_completed {
                    return Err("This model did not complete the Responses tool-call probe. Select a coding model with tool support.".to_string());
                }
                if let Ok(mut verified) = VERIFIED_KEYS.get_or_init(Mutex::default).lock() {
                    if verified.len() >= 128 { verified.remove(0); }
                    verified.push((identity, Instant::now()));
                }
                return Ok(());
            }
        }
        Err("Provider stream ended without a completed response".to_string())
    })
    .await
    .map_err(|_| {
        "Provider validation timed out; the previous configuration was preserved".to_string()
    })?
}

fn setup_error(error: codex_api::ApiError) -> String {
    match error {
        codex_api::ApiError::Transport(codex_api::TransportError::Http { status, .. }) => {
            format!("Provider returned HTTP {status}. Check the key, site, model access and quota.")
        }
        _ => "Provider did not return a valid completed Responses stream. Check model compatibility and connectivity.".to_string(),
    }
}

#[cfg(test)]
#[path = "responses_setup_tests.rs"]
mod tests;

/// Fetch the vendor's ordinary OpenAI-shaped directory, not Codex's metadata API.
pub async fn discover_responses_models(
    factory: HttpClientFactory,
    preset: &ResponsesProviderPreset,
    key: &str,
) -> Result<ModelsResponse, String> {
    if !preset.discovery {
        return Err(
            "This site has no supported model discovery endpoint; showing recommended models"
                .to_string(),
        );
    }
    let url = format!("{}/models", preset.base_url);
    let client = setup_client(factory, url.clone()).await?;
    let mut response = client
        .get(url)
        .bearer_auth(key)
        .timeout(Duration::from_secs(/*secs*/ 15))
        .send()
        .await
        .map_err(|_| "Unable to load provider models".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Model discovery returned HTTP {}",
            response.status()
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Model directory download failed".to_string())?
    {
        if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
            return Err("Provider model directory exceeds 4 MiB".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    let body: Value =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid model directory".to_string())?;
    let entries = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or("Provider did not return a model list")?;
    let mut catalog = crate::recommended_responses_models(preset);
    for entry in entries.iter().take(4096) {
        let Some(id) = entry.get("id").and_then(Value::as_str) else {
            continue;
        };
        if id.is_empty()
            || id.len() > 256
            || id.chars().any(char::is_control)
            || catalog.models.iter().any(|model| model.slug == id)
        {
            continue;
        }
        if preset.family == "HY" && !id.starts_with("hy") {
            continue;
        }
        if preset.family == "OpenRouter" {
            let tools = entry
                .get("supported_parameters")
                .and_then(Value::as_array)
                .map(|params| params.iter().any(|param| param == "tools"));
            let text = entry
                .pointer("/architecture/output_modalities")
                .and_then(Value::as_array)
                .map(|modalities| modalities.iter().any(|modality| modality == "text"));
            let text_input = entry
                .pointer("/architecture/input_modalities")
                .and_then(Value::as_array)
                .map(|modalities| modalities.iter().any(|modality| modality == "text"));
            // Missing capability data stays conservative and is probed before selection.
            if tools == Some(false) || text == Some(false) || text_input == Some(false) {
                continue;
            }
        }
        let mut model = hosted_responses_model(id);
        if let Some(context) = entry
            .get("context_length")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
        {
            model.context_window = Some(context);
            model.max_context_window = Some(context);
        }
        if entry
            .pointer("/architecture/input_modalities")
            .and_then(Value::as_array)
            .is_some_and(|modalities| modalities.iter().any(|modality| modality == "image"))
        {
            model.input_modalities = vec![InputModality::Text, InputModality::Image];
        }
        model.priority = catalog.models.len() as i32;
        catalog.models.push(model);
    }
    Ok(catalog)
}
