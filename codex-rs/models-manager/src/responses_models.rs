//! Conservative model metadata for hosted Responses providers.

use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::openai_models::ApplyPatchToolType;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;

/// Build a Responses model descriptor without assuming OpenAI backend features.
/// Unknown discovered models stay conservative until remote metadata enriches them.
pub fn hosted_responses_model(slug: &str) -> ModelInfo {
    let mut model = crate::model_info::default_model_info(slug);
    model.visibility = ModelVisibility::List;
    model.used_fallback_model_metadata = false;
    model.priority = 0;
    model.default_reasoning_summary = ReasoningSummary::None;
    model.supports_reasoning_summary_parameter = false;
    model.context_window = Some(32_768);
    model.input_modalities = vec![InputModality::Text];
    model.auto_review_model_override = Some(slug.to_string());

    let efforts: &[ReasoningEffort] = match slug {
        "deepseek-flash" | "deepseek-v4-pro" => {
            model.context_window = Some(1_048_576);
            model.apply_patch_tool_type = Some(ApplyPatchToolType::Freeform);
            if slug == "deepseek-flash" {
                model.input_modalities.push(InputModality::Image);
            }
            &[ReasoningEffort::High, ReasoningEffort::Max]
        }
        "glm-5.3" => {
            model.context_window = Some(1_048_576);
            model.apply_patch_tool_type = Some(ApplyPatchToolType::Freeform);
            &[
                ReasoningEffort::Low,
                ReasoningEffort::High,
                ReasoningEffort::Max,
            ]
        }
        "glm-5-turbo" => {
            model.context_window = Some(204_800);
            model.apply_patch_tool_type = Some(ApplyPatchToolType::Freeform);
            &[]
        }
        "kimi-for-coding" | "k3-256k" | "k3" => {
            // K3's 1M window depends on the subscription. 256K works across plans.
            model.context_window = Some(262_144);
            model.input_modalities.push(InputModality::Image);
            &[
                ReasoningEffort::Low,
                ReasoningEffort::High,
                ReasoningEffort::Max,
            ]
        }
        "MiniMax-M3" => {
            model.context_window = Some(1_000_000);
            model.input_modalities.push(InputModality::Image);
            &[ReasoningEffort::None, ReasoningEffort::High]
        }
        _ => &[],
    };
    model.max_context_window = model.context_window;
    if !efforts.is_empty() {
        model.supports_reasoning_summary_parameter = true;
        model.default_reasoning_level = Some(ReasoningEffort::High);
        model.supported_reasoning_levels = efforts
            .iter()
            .map(|effort| ReasoningEffortPreset {
                effort: effort.clone(),
                description: effort.as_str().to_string(),
            })
            .collect();
    }
    model
}
