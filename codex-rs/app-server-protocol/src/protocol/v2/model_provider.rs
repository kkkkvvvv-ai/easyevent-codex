use crate::JsonSchema;
use crate::TS;
use serde::Deserialize;
use serde::Serialize;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ModelProviderListParams {
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ModelProviderInfo {
    pub id: String,
    pub family: String,
    pub name: String,
    pub base_url: String,
    pub key_instructions: String,
    pub configured: bool,
    pub conflict: bool,
    pub recommended_models: Vec<String>,
    pub supports_model_discovery: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ModelProviderListResponse {
    pub data: Vec<ModelProviderInfo>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ModelProviderConfigureParams {
    pub provider_id: String,
    pub api_key: String,
    #[ts(optional = nullable)]
    pub model: Option<String>,
}

impl std::fmt::Debug for ModelProviderConfigureParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelProviderConfigureParams")
            .field("provider_id", &self.provider_id)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .finish()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ModelProviderConfigureResponse {
    pub provider_id: String,
    pub model: String,
    pub configured: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ModelProviderCredentialDeleteParams {
    pub provider_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ModelProviderCredentialDeleteResponse {
    pub deleted: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum ModelCatalogMode {
    Recommended,
    All,
}
