use super::*;

#[test]
fn provider_onboarding_states_never_render_the_key() {
    let provider = ModelProviderInfo {
        id: "kimi".into(),
        family: "Kimi".into(),
        name: "Kimi Code".into(),
        base_url: "https://api.kimi.com/coding/v1".into(),
        key_instructions: "Use a Kimi Code API key.".into(),
        configured: false,
        conflict: false,
        recommended_models: vec!["kimi-for-coding".into(), "k3".into()],
        supports_model_discovery: true,
    };
    let states = [
        ProviderOnboardingState::Sites {
            data: vec![provider.clone()],
            selected: 0,
        },
        ProviderOnboardingState::Key {
            provider: provider.clone(),
            value: "secret-key".into(),
        },
        ProviderOnboardingState::Model {
            provider,
            key: Some("secret-key".into()),
            selected: 0,
        },
        ProviderOnboardingState::Verifying(Uuid::nil()),
    ];
    let renders = states
        .iter()
        .map(|state| {
            let area = Rect::new(
                /*x*/ 0, /*y*/ 0, /*width*/ 72, /*height*/ 10,
            );
            let mut buffer = Buffer::empty(area);
            state.render(
                area,
                &mut buffer,
                Some("Example: model access was denied".into()),
            );
            (0..area.height)
                .map(|y| {
                    (0..area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join("\n")
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");
    assert!(!renders.contains("secret-key"));
    insta::assert_snapshot!("provider_onboarding_states", renders);
}
