use super::*;
use crossterm::event::KeyModifiers;
use pretty_assertions::assert_eq;

#[test]
fn pasted_key_is_masked_and_cancelled_without_an_event() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let provider = ModelProviderInfo {
        id: "deepseek".into(),
        family: "DeepSeek".into(),
        name: "DeepSeek".into(),
        base_url: "https://api.deepseek.com".into(),
        key_instructions: "Paste your DeepSeek API key".into(),
        configured: false,
        conflict: false,
        recommended_models: vec!["deepseek-flash".into()],
        supports_model_discovery: true,
    };
    let mut view = ProviderKeyView::new(provider, AppEventSender::new(tx));
    view.handle_paste("super-secret-key".into());
    let width = 64;
    let area = Rect::new(
        /*x*/ 0,
        /*y*/ 0,
        width,
        view.desired_height(width),
    );
    let mut buffer = Buffer::empty(area);
    view.render(area, &mut buffer);
    let rendered = (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!rendered.contains("super-secret-key"));
    insta::assert_snapshot!("provider_api_key_masked", rendered);
    view.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(view.is_complete());
    assert_eq!(view.value, "");
    assert!(rx.try_recv().is_err());
}
