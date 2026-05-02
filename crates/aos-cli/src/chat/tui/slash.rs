use anyhow::Result;
use aos_agent::ReasoningEffort;

use crate::chat::protocol::parse_reasoning_effort;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SlashCommand {
    Help,
    NewSession,
    Sessions,
    Resume(Option<String>),
    Quit,
    Model(Option<String>),
    Provider(Option<String>),
    Effort(SlashEffort),
    MaxTokens(SlashMaxTokens),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlashCommandKind {
    Help,
    NewSession,
    Sessions,
    Resume,
    Model,
    Provider,
    Effort,
    MaxTokens,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SlashEffort {
    Pick,
    Set(Option<ReasoningEffort>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SlashMaxTokens {
    Pick,
    Set(Option<u64>),
}

pub(crate) fn parse_slash_command(text: &str) -> Result<Option<SlashCommand>> {
    let trimmed = text.trim();
    let Some(command_line) = trimmed.strip_prefix('/') else {
        return Ok(None);
    };
    let command_line = command_line.trim();
    if command_line.is_empty() {
        anyhow::bail!("empty slash command");
    }

    let (name, rest) = split_command(command_line);
    let Some(kind) = SlashCommandKind::from_name(name) else {
        anyhow::bail!("unknown slash command /{name}");
    };
    let command = kind.command_with_args(rest)?;
    Ok(Some(command))
}

pub(crate) fn slash_query(text: &str) -> Option<&str> {
    if !text.starts_with('/') {
        return None;
    }
    let first_line = text.lines().next().unwrap_or(text);
    let query = first_line.strip_prefix('/')?;
    if query.chars().any(char::is_whitespace) {
        return None;
    }
    Some(query)
}

pub(crate) fn matching_slash_commands(query: &str) -> Vec<SlashCommandKind> {
    SlashCommandKind::all()
        .iter()
        .copied()
        .filter(|command| command.name().starts_with(query))
        .collect()
}

impl SlashCommandKind {
    pub(crate) fn all() -> &'static [SlashCommandKind] {
        &[
            SlashCommandKind::Model,
            SlashCommandKind::Sessions,
            SlashCommandKind::NewSession,
            SlashCommandKind::Resume,
            SlashCommandKind::Provider,
            SlashCommandKind::Effort,
            SlashCommandKind::MaxTokens,
            SlashCommandKind::Help,
            SlashCommandKind::Quit,
        ]
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            SlashCommandKind::Help => "help",
            SlashCommandKind::NewSession => "new",
            SlashCommandKind::Sessions => "sessions",
            SlashCommandKind::Resume => "resume",
            SlashCommandKind::Model => "model",
            SlashCommandKind::Provider => "provider",
            SlashCommandKind::Effort => "effort",
            SlashCommandKind::MaxTokens => "max-tokens",
            SlashCommandKind::Quit => "quit",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            SlashCommandKind::Help => "show available chat commands",
            SlashCommandKind::NewSession => "start a fresh session",
            SlashCommandKind::Sessions => "choose a known session",
            SlashCommandKind::Resume => "resume a known session",
            SlashCommandKind::Model => "choose the model for future runs",
            SlashCommandKind::Provider => "choose the LLM provider",
            SlashCommandKind::Effort => "choose thinking effort for future runs",
            SlashCommandKind::MaxTokens => "choose max output tokens",
            SlashCommandKind::Quit => "exit chat",
        }
    }

    pub(crate) fn command_without_args(self) -> SlashCommand {
        match self {
            SlashCommandKind::Help => SlashCommand::Help,
            SlashCommandKind::NewSession => SlashCommand::NewSession,
            SlashCommandKind::Sessions => SlashCommand::Sessions,
            SlashCommandKind::Resume => SlashCommand::Resume(None),
            SlashCommandKind::Model => SlashCommand::Model(None),
            SlashCommandKind::Provider => SlashCommand::Provider(None),
            SlashCommandKind::Effort => SlashCommand::Effort(SlashEffort::Pick),
            SlashCommandKind::MaxTokens => SlashCommand::MaxTokens(SlashMaxTokens::Pick),
            SlashCommandKind::Quit => SlashCommand::Quit,
        }
    }

    fn command_with_args(self, args: &str) -> Result<SlashCommand> {
        Ok(match self {
            SlashCommandKind::Help => SlashCommand::Help,
            SlashCommandKind::NewSession => SlashCommand::NewSession,
            SlashCommandKind::Sessions => SlashCommand::Sessions,
            SlashCommandKind::Resume => SlashCommand::Resume(optional_value(args)),
            SlashCommandKind::Quit => SlashCommand::Quit,
            SlashCommandKind::Model => SlashCommand::Model(optional_value(args)),
            SlashCommandKind::Provider => SlashCommand::Provider(optional_value(args)),
            SlashCommandKind::Effort => {
                if args.is_empty() {
                    SlashCommand::Effort(SlashEffort::Pick)
                } else {
                    SlashCommand::Effort(SlashEffort::Set(parse_reasoning_effort(args)?))
                }
            }
            SlashCommandKind::MaxTokens => {
                if args.is_empty() {
                    SlashCommand::MaxTokens(SlashMaxTokens::Pick)
                } else {
                    SlashCommand::MaxTokens(SlashMaxTokens::Set(parse_max_tokens(args)?))
                }
            }
        })
    }

    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "help" | "?" => SlashCommandKind::Help,
            "new" => SlashCommandKind::NewSession,
            "sessions" | "session" => SlashCommandKind::Sessions,
            "resume" => SlashCommandKind::Resume,
            "quit" | "exit" => SlashCommandKind::Quit,
            "model" => SlashCommandKind::Model,
            "provider" => SlashCommandKind::Provider,
            "effort" | "thinking" => SlashCommandKind::Effort,
            "max-tokens" | "tokens" => SlashCommandKind::MaxTokens,
            _ => return None,
        })
    }
}

fn split_command(command_line: &str) -> (&str, &str) {
    let Some((idx, _)) = command_line
        .char_indices()
        .find(|(_, ch)| ch.is_ascii_whitespace())
    else {
        return (command_line, "");
    };
    (&command_line[..idx], command_line[idx..].trim())
}

fn optional_value(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn parse_max_tokens(value: &str) -> Result<Option<u64>> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "none" | "off" | "default" => Ok(None),
        _ => Ok(Some(value.trim().parse()?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_picker_commands_without_arguments() {
        assert_eq!(
            parse_slash_command("/model").unwrap(),
            Some(SlashCommand::Model(None))
        );
        assert_eq!(
            parse_slash_command("/effort").unwrap(),
            Some(SlashCommand::Effort(SlashEffort::Pick))
        );
        assert_eq!(
            parse_slash_command("/sessions").unwrap(),
            Some(SlashCommand::Sessions)
        );
        assert_eq!(
            parse_slash_command("/resume").unwrap(),
            Some(SlashCommand::Resume(None))
        );
    }

    #[test]
    fn parses_direct_setting_commands() {
        assert_eq!(
            parse_slash_command("/model gpt-5.3-codex").unwrap(),
            Some(SlashCommand::Model(Some("gpt-5.3-codex".into())))
        );
        assert_eq!(
            parse_slash_command("/effort high").unwrap(),
            Some(SlashCommand::Effort(SlashEffort::Set(Some(
                ReasoningEffort::High
            ))))
        );
        assert_eq!(
            parse_slash_command("/max-tokens none").unwrap(),
            Some(SlashCommand::MaxTokens(SlashMaxTokens::Set(None)))
        );
        assert_eq!(
            parse_slash_command("/resume 018f2a66-31cc-7b25-a4f7-37e3310fdc6c").unwrap(),
            Some(SlashCommand::Resume(Some(
                "018f2a66-31cc-7b25-a4f7-37e3310fdc6c".into()
            )))
        );
    }

    #[test]
    fn non_slash_input_is_not_a_command() {
        assert_eq!(parse_slash_command("hello").unwrap(), None);
    }

    #[test]
    fn slash_query_only_matches_first_token() {
        assert_eq!(slash_query("/mo"), Some("mo"));
        assert_eq!(slash_query("/model extra"), None);
        assert_eq!(slash_query("hello /mo"), None);
    }

    #[test]
    fn command_matches_filter_by_prefix() {
        assert_eq!(matching_slash_commands("mo"), vec![SlashCommandKind::Model]);
        assert_eq!(
            matching_slash_commands("se"),
            vec![SlashCommandKind::Sessions]
        );
        assert!(matching_slash_commands("").contains(&SlashCommandKind::Provider));
    }
}
