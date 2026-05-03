use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::{Args, Subcommand};
use serde_json::json;

use crate::GlobalOpts;
use crate::chat::config::{
    cached_selected_session, load_default_draft_settings, save_selected_session,
};
use crate::chat::protocol::{ChatDraftOverrideMask, ChatToolMode};
use crate::chat::tui::{ChatTuiShellOptions, run_shell};
use crate::chat::{
    ChatControlClient, ChatDraftSettings, ChatSessionDriver, ChatSessionDriverOptions,
    parse_reasoning_effort,
};
use crate::client::ApiClient;
use crate::commands::common::{resolve_target, resolve_world_arg_or_selected};
use crate::output::{OutputOpts, print_success};

#[derive(Args, Debug)]
#[command(about = "Chat with an AgentOS agent session")]
pub(crate) struct ChatArgs {
    #[command(subcommand)]
    cmd: Option<ChatCommandArgs>,

    #[command(flatten)]
    open: ChatOpenArgs,
}

#[derive(Subcommand, Debug)]
enum ChatCommandArgs {
    /// List known agent sessions in the selected world.
    Sessions(ChatSessionsArgs),
    /// Render reconstructed session history as data.
    History(ChatHistoryArgs),
}

#[derive(Args, Debug, Clone, Default)]
struct ChatOpenArgs {
    /// World ID. Defaults to the selected world.
    #[arg(long)]
    world: Option<String>,
    /// Session ID to resume.
    #[arg(long)]
    session: Option<String>,
    /// Start with a fresh session ID.
    #[arg(long)]
    new: bool,
    /// Journal sequence to start following from.
    #[arg(long)]
    from: Option<u64>,
    /// Draft provider for a new session.
    #[arg(long)]
    provider: Option<String>,
    /// Draft model for a new session.
    #[arg(long)]
    model: Option<String>,
    /// Draft reasoning effort: low, medium, high, or none.
    #[arg(long)]
    effort: Option<String>,
    /// Draft max output token limit.
    #[arg(long)]
    max_tokens: Option<u64>,
    /// Tool surface to install when a chat session has no tools.
    #[arg(long, value_enum, default_value = "local-coding")]
    tools: ChatToolMode,
    /// Working directory for local coding tools. Defaults to the current directory.
    #[arg(long)]
    workdir: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct ChatSessionsArgs {
    /// World ID. Defaults to the selected world.
    #[arg(long)]
    world: Option<String>,
}

#[derive(Args, Debug)]
struct ChatHistoryArgs {
    /// Session ID to read.
    #[arg(long)]
    session: String,
    /// World ID. Defaults to the selected world.
    #[arg(long)]
    world: Option<String>,
    /// Emit the reconstructed turns as JSON, regardless of global output mode.
    #[arg(long)]
    json: bool,
}

pub(crate) async fn handle(global: &GlobalOpts, output: OutputOpts, args: ChatArgs) -> Result<()> {
    match args.cmd {
        Some(ChatCommandArgs::Sessions(args)) => handle_sessions(global, output, args).await,
        Some(ChatCommandArgs::History(args)) => handle_history(global, output, args).await,
        None => handle_open(global, output, args.open).await,
    }
}

async fn handle_sessions(
    global: &GlobalOpts,
    output: OutputOpts,
    args: ChatSessionsArgs,
) -> Result<()> {
    let (client, _) = chat_client(global, args.world.as_deref()).await?;
    let sessions = client.list_sessions().await?;
    print_success(output, json!(sessions), None, vec![])
}

async fn handle_history(
    global: &GlobalOpts,
    output: OutputOpts,
    args: ChatHistoryArgs,
) -> Result<()> {
    let (client, world_id) = chat_client(global, args.world.as_deref()).await?;
    let draft = load_default_draft_settings(global)?;
    let (driver, _) = ChatSessionDriver::open(
        client,
        ChatSessionDriverOptions {
            session_id: args.session,
            draft_settings: draft,
            draft_overrides: ChatDraftOverrideMask::default(),
            tool_mode: ChatToolMode::None,
            workdir: std::env::current_dir()
                .context("resolve current directory")?
                .to_string_lossy()
                .into_owned(),
            from: None,
        },
    )
    .await?;
    save_selected_session(global, &world_id, driver.session_id())?;
    let mut output = output;
    if args.json {
        output.json = true;
    }
    print_success(output, json!(driver.turns()), None, vec![])
}

async fn handle_open(global: &GlobalOpts, output: OutputOpts, args: ChatOpenArgs) -> Result<()> {
    let (client, world_id) = chat_client(global, args.world.as_deref()).await?;
    let mut draft = load_default_draft_settings(global)?;
    let draft_overrides = apply_draft_overrides(
        &mut draft,
        args.provider,
        args.model,
        args.effort.as_deref(),
        args.max_tokens,
    )?;
    let workdir = resolve_chat_workdir(args.workdir)?;
    let session_id = resolve_session_id(global, &client, &world_id, args.session, args.new).await?;

    if output.json || output.pretty {
        return print_success(
            output,
            json!({
                "world_id": world_id,
                "session_id": session_id,
                "ui": "tui",
            }),
            None,
            vec![],
        );
    }

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(anyhow!(
            "aos chat is TUI-only and requires an interactive terminal"
        ));
    }
    save_selected_session(global, &world_id, &session_id)?;
    run_shell(ChatTuiShellOptions {
        client,
        session_id,
        draft_settings: draft,
        draft_overrides,
        tool_mode: args.tools,
        workdir,
        from: args.from,
    })
    .await
}

async fn chat_client(
    global: &GlobalOpts,
    world: Option<&str>,
) -> Result<(ChatControlClient, String)> {
    let target = resolve_target(global)?;
    let world_id = resolve_world_arg_or_selected(&target, world)?;
    let api = ApiClient::new(&target)?;
    Ok((ChatControlClient::new(api, world_id.clone()), world_id))
}

async fn resolve_session_id(
    global: &GlobalOpts,
    client: &ChatControlClient,
    world_id: &str,
    requested: Option<String>,
    new: bool,
) -> Result<String> {
    if new {
        return Ok(crate::chat::session::new_session_id());
    }
    if let Some(session_id) = requested {
        return crate::chat::session::validate_session_id(&session_id);
    }
    if let Some(session_id) = cached_selected_session(global, world_id)? {
        return crate::chat::session::validate_session_id(&session_id);
    }
    if let Some(summary) = client.list_sessions().await?.into_iter().next() {
        return Ok(summary.session_id);
    }
    Ok(crate::chat::session::new_session_id())
}

fn apply_draft_overrides(
    draft: &mut ChatDraftSettings,
    provider: Option<String>,
    model: Option<String>,
    effort: Option<&str>,
    max_tokens: Option<u64>,
) -> Result<ChatDraftOverrideMask> {
    let mut mask = ChatDraftOverrideMask::default();
    if let Some(provider) = provider {
        draft.provider = provider;
        mask.provider = true;
    }
    if let Some(model) = model {
        draft.model = model;
        mask.model = true;
    }
    if let Some(effort) = effort {
        draft.reasoning_effort = parse_reasoning_effort(effort)?;
        mask.reasoning_effort = true;
    }
    if let Some(max_tokens) = max_tokens {
        draft.max_tokens = Some(max_tokens);
        mask.max_tokens = true;
    }
    Ok(mask)
}

fn resolve_chat_workdir(workdir: Option<PathBuf>) -> Result<String> {
    let path = match workdir {
        Some(path) if path.is_absolute() => path,
        Some(path) => std::env::current_dir()
            .context("resolve current directory")?
            .join(path),
        None => std::env::current_dir().context("resolve current directory")?,
    };
    let path = path
        .canonicalize()
        .with_context(|| format!("resolve chat workdir '{}'", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}
