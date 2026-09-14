//! Adapted from codex-plus f8a2db762fc6 (Apache-2.0). Stable account identity
//! excludes refreshed access tokens. Only the digest may be persisted.
use crate::ModelProvider;
use codex_login::CodexAuth;
use codex_protocol::error::Result;
use sha2::Digest;
use sha2::Sha256;

/// Identity of a provider's routing and effective credentials, independent of model IDs.
pub async fn model_provider_identity(
    provider: &dyn ModelProvider,
    provider_id: &str,
) -> Result<String> {
    let info = provider.info();
    let auth = provider.auth().await;
    let mut api = provider.api_provider().await?;
    let mut digest = Sha256::new();
    let mut field = |value: &[u8]| {
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value);
    };
    field(b"provider-runtime-v2");
    field(provider_id.as_bytes());
    field(info.wire_api.to_string().as_bytes());
    field(api.base_url.as_bytes());
    let mut query: Vec<_> = api.query_params.iter().flatten().collect();
    query.sort();
    for (name, value) in query {
        field(name.as_bytes());
        field(value.as_bytes());
    }
    let stable_account = info.requires_openai_auth
        && info.env_key.is_none()
        && info.experimental_bearer_token.is_none()
        && info.auth.is_none()
        && auth.as_ref().is_some_and(|auth| {
            matches!(
                auth,
                CodexAuth::Chatgpt(_)
                    | CodexAuth::ChatgptAuthTokens(_)
                    | CodexAuth::AgentIdentity(_)
            ) && auth.get_account_id().is_some()
                && (auth.get_chatgpt_user_id().is_some() || auth.get_account_email().is_some())
        });
    if stable_account {
        if let Some(auth) = &auth {
            field(format!("{:?}", auth.get_account_id()).as_bytes());
            field(format!("{:?}", auth.get_chatgpt_user_id()).as_bytes());
            if auth.get_chatgpt_user_id().is_none() {
                field(format!("{:?}", auth.get_account_email()).as_bytes());
            }
            field(&[
                u8::from(auth.is_fedramp_account()),
                u8::from(auth.is_workspace_account()),
            ]);
        }
    } else {
        api.headers
            .extend(provider.api_auth().await?.to_auth_headers());
    }
    let mut headers: Vec<_> = api.headers.iter().collect();
    headers.sort_by(|(a, av), (b, bv)| {
        a.as_str()
            .cmp(b.as_str())
            .then(av.as_bytes().cmp(bv.as_bytes()))
    });
    for (name, value) in headers {
        field(name.as_str().as_bytes());
        field(value.as_bytes());
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
#[path = "runtime_identity_tests.rs"]
mod tests;
