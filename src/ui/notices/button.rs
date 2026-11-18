use crate::tmux::CODEX_AGENT;

/// Whether the missing-hooks section should render a `[copy]` button
/// next to `agent`. Only Codex qualifies — Claude's setup story is
/// owned by the dedicated `Plugin / claude` section (which has its own
/// `[prompt]` button), so adding a second clickable copy target on the
/// Claude row would race with it and flip the shared `[copied]` feedback
/// state for both buttons at once.
///
/// Kept as a pure check so layout calculations do not pay the cost of
/// resolving the running binary path on every frame.
pub(super) fn missing_hooks_has_copy_button(agent: &str) -> bool {
    agent == CODEX_AGENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_hooks_has_copy_button_only_for_codex() {
        // Claude is excluded because the Plugin / claude section owns
        // its own [prompt] button — leaving a [copy] on the Claude row
        // would race with it on the shared `[copied]` feedback state.
        assert!(missing_hooks_has_copy_button("codex"));
        assert!(!missing_hooks_has_copy_button("claude"));
        assert!(!missing_hooks_has_copy_button("gemini"));
        assert!(!missing_hooks_has_copy_button(""));
    }
}
