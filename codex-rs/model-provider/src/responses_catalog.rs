//! Bounded model catalogs scoped to the endpoint and credential identity.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;

use codex_login::auth::ProviderCredentialStore;
use codex_model_provider_info::ResponsesProviderPreset;
use codex_protocol::openai_models::ModelsResponse;
use sha2::Digest;
use sha2::Sha256;

type Catalogs = HashMap<String, (String, ModelsResponse)>;
static CATALOGS: OnceLock<Mutex<Catalogs>> = OnceLock::new();

pub fn cached_responses_models(
    store: &ProviderCredentialStore,
    preset: &ResponsesProviderPreset,
    key: &str,
) -> ModelsResponse {
    let identity = format!(
        "{:x}",
        Sha256::digest(format!("{}\0{key}", preset.base_url).as_bytes())
    );
    if let Ok(catalogs) = CATALOGS.get_or_init(Mutex::default).lock()
        && let Some((cached_identity, catalog)) = catalogs.get(preset.id)
        && cached_identity == &identity
    {
        return catalog.clone();
    }
    if let Some(directory) = store.model_cache_directory(preset) {
        let path = directory.join(format!("{identity}.json"));
        if std::fs::metadata(&path).is_ok_and(|meta| meta.len() <= 4 * 1024 * 1024)
            && let Ok(bytes) = std::fs::read(path)
            && let Ok(catalog) = serde_json::from_slice::<ModelsResponse>(&bytes)
        {
            return catalog;
        }
    }
    crate::recommended_responses_models(preset)
}

pub fn cache_responses_models(
    store: &ProviderCredentialStore,
    preset: &ResponsesProviderPreset,
    key: &str,
    catalog: ModelsResponse,
) -> std::io::Result<()> {
    let identity = format!(
        "{:x}",
        Sha256::digest(format!("{}\0{key}", preset.base_url).as_bytes())
    );
    let bytes = serde_json::to_vec(&catalog)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(std::io::Error::other("Model catalog exceeds 4 MiB"));
    }
    let mut catalogs = CATALOGS
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| std::io::Error::other("Model cache unavailable"))?;
    if let Some(directory) = store.model_cache_directory(preset) {
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{identity}.json"));
        let temporary = directory.join(format!("{identity}.{}.tmp", std::process::id()));
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(temporary, path)?;
    }
    // At most one credential identity per built-in site is retained in memory.
    catalogs.insert(preset.id.to_string(), (identity, catalog));
    Ok(())
}
