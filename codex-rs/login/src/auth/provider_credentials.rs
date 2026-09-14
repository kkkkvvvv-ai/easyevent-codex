//! Provider credentials use the existing auth backends in an isolated namespace.
//! OpenAI login/logout cannot overwrite these records. The namespace includes the
//! canonical endpoint, so a changed endpoint cannot acquire a saved credential.

use std::path::PathBuf;

use codex_model_provider_info::ResponsesProviderPreset;
use sha2::Digest;
use sha2::Sha256;

use super::AuthKeyringBackendKind;
use super::load_auth_dot_json;
use super::login_with_api_key;
use super::logout;
use crate::AuthCredentialsStoreMode;

#[derive(Clone, Debug)]
pub struct ProviderCredentialStore {
    codex_home: PathBuf,
    mode: AuthCredentialsStoreMode,
    backend: AuthKeyringBackendKind,
}

impl ProviderCredentialStore {
    pub fn new(
        codex_home: PathBuf,
        mode: AuthCredentialsStoreMode,
        backend: AuthKeyringBackendKind,
    ) -> Self {
        Self {
            codex_home,
            mode,
            backend,
        }
    }

    fn namespace(&self, preset: &ResponsesProviderPreset) -> PathBuf {
        let endpoint = format!("{:x}", Sha256::digest(preset.base_url.as_bytes()));
        self.codex_home
            .join("provider-auth")
            .join(preset.id)
            .join(endpoint)
    }

    /// Persistent metadata may accompany credentials, except in ephemeral mode.
    pub fn model_cache_directory(&self, preset: &ResponsesProviderPreset) -> Option<PathBuf> {
        (self.mode != AuthCredentialsStoreMode::Ephemeral)
            .then(|| self.namespace(preset).join("models"))
    }

    pub fn read(&self, preset: &ResponsesProviderPreset) -> std::io::Result<Option<String>> {
        Ok(
            load_auth_dot_json(&self.namespace(preset), self.mode, self.backend)?
                .and_then(|auth| auth.openai_api_key),
        )
    }

    pub fn write(&self, preset: &ResponsesProviderPreset, key: &str) -> std::io::Result<()> {
        let key = key.trim();
        if key.is_empty() || key.chars().any(char::is_control) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "API key must be nonempty and contain no control characters",
            ));
        }
        login_with_api_key(&self.namespace(preset), key, self.mode, self.backend)
    }

    pub fn delete(&self, preset: &ResponsesProviderPreset) -> std::io::Result<bool> {
        logout(&self.namespace(preset), self.mode, self.backend)
    }
}

#[cfg(test)]
#[path = "provider_credentials_tests.rs"]
mod tests;
