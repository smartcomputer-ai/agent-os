use crate::chat::protocol::{ChatPromptConfig, ChatPromptProfile, ChatToolMode};

pub(crate) const LOCAL_CODING_PROMPT: &str = "You are an AOS local coding agent. Use tools to inspect files and run commands when needed. Prefer small focused edits, keep explanations concise, and do not claim a command succeeded unless tool results show it.";

pub(crate) fn selected_prompt_text(
    config: &ChatPromptConfig,
    tool_mode: ChatToolMode,
) -> Option<&str> {
    match config {
        ChatPromptConfig::Auto if matches!(tool_mode, ChatToolMode::LocalCoding) => {
            Some(LOCAL_CODING_PROMPT)
        }
        ChatPromptConfig::Auto | ChatPromptConfig::None => None,
        ChatPromptConfig::Profile(ChatPromptProfile::None) => None,
        ChatPromptConfig::Profile(ChatPromptProfile::LocalCoding) => Some(LOCAL_CODING_PROMPT),
        ChatPromptConfig::Inline(text) if text.trim().is_empty() => None,
        ChatPromptConfig::Inline(text) => Some(text.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_prompt_selects_local_coding_only_for_local_coding_tools() {
        assert_eq!(
            selected_prompt_text(&ChatPromptConfig::Auto, ChatToolMode::LocalCoding),
            Some(LOCAL_CODING_PROMPT)
        );
        assert_eq!(
            selected_prompt_text(&ChatPromptConfig::Auto, ChatToolMode::Workspace),
            None
        );
    }

    #[test]
    fn inline_prompt_overrides_profile_selection() {
        assert_eq!(
            selected_prompt_text(
                &ChatPromptConfig::Inline("custom".into()),
                ChatToolMode::None
            ),
            Some("custom")
        );
    }
}
