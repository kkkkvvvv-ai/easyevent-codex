use super::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;

/// Bounded request for a readable continuation; it never grants tool authority.
pub(crate) struct ProviderHandoffRequest {
    pub(crate) token_limit: usize,
}

impl ContextualUserFragment for ProviderHandoffRequest {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("compaction.provider_handoff".into())
    }
    fn role(&self) -> &'static str {
        "user"
    }
    fn markers(&self) -> (&'static str, &'static str) {
        ("", "")
    }
    fn type_markers() -> (&'static str, &'static str) {
        ("", "")
    }
    fn body(&self) -> String {
        let limit = self.token_limit.min(4096);
        format!(
            "Prepare a readable continuation summary for another model, within {limit} tokens and 16384 UTF-8 bytes. Preserve the user's objective, constraints, accepted plan and decisions, completed file changes, tool results and checks, review findings with file locations, and remaining work. Clearly distinguish completed work from pending work. Do not execute tools, continue the task, include credentials, or copy opaque encrypted state. Return a complete summary rather than an unfinished fragment."
        )
    }
}
