//! Hosted Responses endpoints offered by the provider setup flow.
//!
//! Each site has its own identity: credentials must never cross site boundaries.

use crate::ModelProviderInfo;
use crate::WireApi;

#[derive(Debug, Clone, Copy)]
pub struct ResponsesProviderPreset {
    pub id: &'static str,
    pub family: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub key_instructions: &'static str,
    pub models: &'static [&'static str],
    pub discovery: bool,
}

pub const RESPONSES_PROVIDER_PRESETS: &[ResponsesProviderPreset] = &[
    ResponsesProviderPreset {
        id: "deepseek",
        family: "DeepSeek",
        name: "DeepSeek",
        base_url: "https://api.deepseek.com",
        key_instructions: "API key from platform.deepseek.com",
        models: &["deepseek-flash", "deepseek-v4-pro"],
        discovery: true,
    },
    ResponsesProviderPreset {
        id: "glm-cn",
        family: "GLM",
        name: "GLM (China)",
        base_url: "https://open.bigmodel.cn/api/v1",
        key_instructions: "GLM Coding Plan key from bigmodel.cn; team keys are separate",
        models: &["glm-5.3", "glm-5-turbo"],
        discovery: false,
    },
    ResponsesProviderPreset {
        id: "glm-intl",
        family: "GLM",
        name: "GLM (Z.AI)",
        base_url: "https://api.z.ai/api/v1",
        key_instructions: "Coding Plan key from z.ai; not interchangeable with China keys",
        models: &["glm-5.3"],
        discovery: false,
    },
    ResponsesProviderPreset {
        id: "kimi",
        family: "Kimi",
        name: "Kimi Code",
        base_url: "https://api.kimi.com/coding/v1",
        key_instructions: "Kimi Code key from kimi.com/code/console, not a Moonshot API key",
        models: &["kimi-for-coding", "k3-256k", "k3"],
        discovery: true,
    },
    ResponsesProviderPreset {
        id: "hy-cn",
        family: "HY",
        name: "HY (TokenHub China)",
        base_url: "https://tokenhub.tencentmaas.com/v1",
        key_instructions: "TokenHub API key for the Guangzhou site",
        models: &["hy4-preview"],
        discovery: true,
    },
    ResponsesProviderPreset {
        id: "hy-intl",
        family: "HY",
        name: "HY (TokenHub International)",
        base_url: "https://tokenhub-intl.tencentcloudmaas.com/v1",
        key_instructions: "TokenHub API key for the international Singapore site",
        models: &["hy4-preview"],
        discovery: true,
    },
    ResponsesProviderPreset {
        id: "minimax-cn",
        family: "MiniMax",
        name: "MiniMax (China)",
        base_url: "https://api.minimax.cn/v1",
        key_instructions: "MiniMax China key; Token Plan and pay-as-you-go keys use separate quotas",
        models: &["MiniMax-M3"],
        discovery: true,
    },
    ResponsesProviderPreset {
        id: "minimax-intl",
        family: "MiniMax",
        name: "MiniMax (International)",
        base_url: "https://api.minimax.io/v1",
        key_instructions: "MiniMax international key; Token Plan and pay-as-you-go keys use separate quotas",
        models: &["MiniMax-M3"],
        discovery: true,
    },
    ResponsesProviderPreset {
        id: "openrouter",
        family: "OpenRouter",
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        key_instructions: "API key from openrouter.ai/keys",
        models: &["~openai/gpt-latest", "~anthropic/claude-sonnet-latest"],
        discovery: true,
    },
];

pub fn responses_provider_preset(id: &str) -> Option<&'static ResponsesProviderPreset> {
    RESPONSES_PROVIDER_PRESETS
        .iter()
        .find(|preset| preset.id == id)
}

/// Recognize only unmodified presets, never a custom URL or auth override.
pub fn responses_preset_for_info(
    info: &ModelProviderInfo,
) -> Option<&'static ResponsesProviderPreset> {
    RESPONSES_PROVIDER_PRESETS
        .iter()
        .find(|preset| preset.provider_info() == *info)
}

impl ResponsesProviderPreset {
    pub fn provider_info(&self) -> ModelProviderInfo {
        ModelProviderInfo {
            name: self.name.to_string(),
            base_url: Some(self.base_url.to_string()),
            wire_api: WireApi::Responses,
            ..Default::default()
        }
    }
}
