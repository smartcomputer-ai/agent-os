use anyhow::{Context, Result};

use crate::GlobalOpts;
use crate::chat::protocol::{ChatDraftSettings, parse_reasoning_effort};
use crate::config::{CliChatWorldConfig, ConfigPaths, load_config, save_config};

pub(crate) fn load_default_draft_settings(global: &GlobalOpts) -> Result<ChatDraftSettings> {
    let paths = ConfigPaths::resolve(global.config.as_deref())?;
    let config = load_config(&paths)?;
    let mut settings = ChatDraftSettings::default();
    if let Some(provider) = config.chat.default_provider {
        settings.provider = provider;
    }
    if let Some(model) = config.chat.default_model {
        settings.model = model;
    }
    if let Some(effort) = config.chat.default_reasoning_effort {
        settings.reasoning_effort = parse_reasoning_effort(&effort)
            .with_context(|| format!("parse configured chat reasoning effort '{effort}'"))?;
    }
    if let Some(max_tokens) = config.chat.default_max_tokens {
        settings.max_tokens = Some(max_tokens);
    }
    Ok(settings)
}

pub(crate) fn cached_selected_session(
    global: &GlobalOpts,
    world_id: &str,
) -> Result<Option<String>> {
    let paths = ConfigPaths::resolve(global.config.as_deref())?;
    let config = load_config(&paths)?;
    Ok(config
        .chat
        .worlds
        .get(world_id)
        .and_then(|world| world.selected_session.clone()))
}

pub(crate) fn save_selected_session(
    global: &GlobalOpts,
    world_id: &str,
    session_id: &str,
) -> Result<()> {
    let paths = ConfigPaths::resolve(global.config.as_deref())?;
    let mut config = load_config(&paths)?;
    config
        .chat
        .worlds
        .entry(world_id.to_string())
        .or_insert_with(CliChatWorldConfig::default)
        .selected_session = Some(session_id.to_string());
    save_config(&paths, &config)
}
