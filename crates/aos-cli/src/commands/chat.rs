use std::io::{IsTerminal, Read, Write};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use clap::{Args, Subcommand};
use serde_json::json;

use crate::GlobalOpts;
use crate::chat::config::{
    cached_selected_session, load_default_draft_settings, save_selected_session,
};
use crate::chat::plain::PlainRenderer;
use crate::chat::protocol::ChatDraftOverrideMask;
use crate::chat::{
    ChatCommand, ChatControlClient, ChatDraftSettings, ChatEngine, ChatEngineOptions,
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
    /// Render reconstructed session history.
    History(ChatHistoryArgs),
    /// Submit one user message.
    Send(ChatSendArgs),
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
    /// Use the line-oriented diagnostic renderer instead of the P3 full-screen TUI.
    #[arg(long)]
    plain: bool,
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

#[derive(Args, Debug)]
struct ChatSendArgs {
    /// Session ID to send to.
    #[arg(long)]
    session: String,
    /// User message text.
    #[arg(long)]
    message: String,
    /// World ID. Defaults to the selected world.
    #[arg(long)]
    world: Option<String>,
    /// Stream plain progress until the submitted run becomes idle/terminal.
    #[arg(long)]
    follow: bool,
    /// Print plain diagnostic lines instead of JSON acceptance details.
    #[arg(long)]
    plain: bool,
    /// Treat the message as a follow-up even if the session is currently idle.
    ///
    /// The chat engine already chooses the workflow-safe input kind, so this flag is
    /// accepted for script compatibility and does not alter first-run detection.
    #[arg(long)]
    follow_input: bool,
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
    /// Follow timeout in milliseconds.
    #[arg(long, default_value_t = 300_000)]
    timeout_ms: u64,
}

pub(crate) async fn handle(global: &GlobalOpts, output: OutputOpts, args: ChatArgs) -> Result<()> {
    match args.cmd {
        Some(ChatCommandArgs::Sessions(args)) => handle_sessions(global, output, args).await,
        Some(ChatCommandArgs::History(args)) => handle_history(global, output, args).await,
        Some(ChatCommandArgs::Send(args)) => handle_send(global, output, args).await,
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
    mut output: OutputOpts,
    args: ChatHistoryArgs,
) -> Result<()> {
    if args.json {
        output.json = true;
    }
    let (client, world_id) = chat_client(global, args.world.as_deref()).await?;
    let draft = load_default_draft_settings(global)?;
    let (engine, events) = ChatEngine::open(
        client,
        ChatEngineOptions {
            session_id: args.session,
            draft_settings: draft,
            draft_overrides: ChatDraftOverrideMask::default(),
            from: None,
        },
    )
    .await?;
    save_selected_session(global, &world_id, engine.session_id())?;
    if output.json || output.pretty {
        return print_success(output, json!(engine.turns()), None, vec![]);
    }
    let mut renderer = PlainRenderer::default();
    renderer.render_events(&events);
    renderer.render_history(engine.turns());
    Ok(())
}

async fn handle_send(global: &GlobalOpts, output: OutputOpts, args: ChatSendArgs) -> Result<()> {
    let (client, world_id) = chat_client(global, args.world.as_deref()).await?;
    let _ = args.follow_input;
    let mut draft = load_default_draft_settings(global)?;
    let draft_overrides = apply_draft_overrides(
        &mut draft,
        args.provider,
        args.model,
        args.effort.as_deref(),
        args.max_tokens,
    )?;
    let (mut engine, mut events) = ChatEngine::open(
        client,
        ChatEngineOptions {
            session_id: args.session,
            draft_settings: draft,
            draft_overrides,
            from: None,
        },
    )
    .await?;
    save_selected_session(global, &world_id, engine.session_id())?;
    events.extend(
        engine
            .handle_command(ChatCommand::SubmitUserMessage { text: args.message })
            .await?,
    );
    if args.follow || args.plain {
        let mut renderer = PlainRenderer::default();
        renderer.render_events(&events);
        if args.follow {
            engine
                .follow_until_quiescent(Duration::from_millis(args.timeout_ms), |event| {
                    renderer.render_event(&event);
                })
                .await?;
        }
        return Ok(());
    }
    print_success(output, json!(events), None, vec![])
}

async fn handle_open(global: &GlobalOpts, output: OutputOpts, args: ChatOpenArgs) -> Result<()> {
    if !args.plain {
        return Err(anyhow!(
            "the full-screen chat TUI is specified in roadmap/v0.24-claw/p3-chat-tui.md and is not implemented yet; use `aos chat --plain`, `aos chat send`, `aos chat sessions`, or `aos chat history` for the P2 engine"
        ));
    }

    let (client, world_id) = chat_client(global, args.world.as_deref()).await?;
    let mut draft = load_default_draft_settings(global)?;
    let draft_overrides = apply_draft_overrides(
        &mut draft,
        args.provider,
        args.model,
        args.effort.as_deref(),
        args.max_tokens,
    )?;
    let session_id = resolve_session_id(global, &client, &world_id, args.session, args.new).await?;
    let (mut engine, events) = ChatEngine::open(
        client,
        ChatEngineOptions {
            session_id,
            draft_settings: draft,
            draft_overrides,
            from: args.from,
        },
    )
    .await?;
    save_selected_session(global, &world_id, engine.session_id())?;

    if output.json || output.pretty {
        return print_success(output, json!(events), None, vec![]);
    }

    let mut renderer = PlainRenderer::default();
    renderer.render_events(&events);

    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        let mut message = String::new();
        stdin
            .lock()
            .read_to_string(&mut message)
            .context("read stdin chat message")?;
        let message = message.trim();
        if !message.is_empty() {
            let events = engine
                .handle_command(ChatCommand::SubmitUserMessage {
                    text: message.to_string(),
                })
                .await?;
            renderer.render_events(&events);
            engine
                .follow_until_quiescent(Duration::from_secs(300), |event| {
                    renderer.render_event(&event);
                })
                .await?;
        }
        return Ok(());
    }

    plain_repl(engine, renderer).await
}

async fn plain_repl(mut engine: ChatEngine, mut renderer: PlainRenderer) -> Result<()> {
    loop {
        print!("> ");
        std::io::stdout().flush().context("flush prompt")?;
        let mut line = String::new();
        let bytes = std::io::stdin()
            .read_line(&mut line)
            .context("read chat input")?;
        if bytes == 0 {
            return Ok(());
        }
        let line = line.trim_end().to_string();
        if line.trim().is_empty() {
            continue;
        }
        if matches!(line.trim(), "/quit" | "/exit") {
            return Ok(());
        }
        if handle_plain_slash(&mut engine, &mut renderer, &line).await? {
            continue;
        }
        let events = engine
            .handle_command(ChatCommand::SubmitUserMessage { text: line })
            .await?;
        renderer.render_events(&events);
        engine
            .follow_until_quiescent(Duration::from_secs(300), |event| {
                renderer.render_event(&event);
            })
            .await?;
    }
}

async fn handle_plain_slash(
    engine: &mut ChatEngine,
    renderer: &mut PlainRenderer,
    line: &str,
) -> Result<bool> {
    let Some(rest) = line.strip_prefix('/') else {
        return Ok(false);
    };
    let mut parts = rest.splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or_default();
    let arg = parts.next().unwrap_or_default().trim();
    let chat_command = match command {
        "quit" | "exit" => return Ok(true),
        "model" if !arg.is_empty() => ChatCommand::SetDraftModel { model: arg.into() },
        "provider" if !arg.is_empty() => ChatCommand::SetDraftProvider {
            provider: arg.into(),
        },
        "effort" => ChatCommand::SetDraftReasoningEffort {
            effort: parse_reasoning_effort(arg)?,
        },
        "max-tokens" if arg == "none" || arg.is_empty() => {
            ChatCommand::SetDraftMaxTokens { max_tokens: None }
        }
        "max-tokens" => ChatCommand::SetDraftMaxTokens {
            max_tokens: Some(arg.parse::<u64>().context("parse /max-tokens value")?),
        },
        "interrupt" => ChatCommand::InterruptRun {
            reason: (!arg.is_empty()).then(|| arg.to_string()),
        },
        "steer" if !arg.is_empty() => ChatCommand::SteerRun { text: arg.into() },
        "pause" => ChatCommand::PauseSession,
        "resume" => ChatCommand::ResumeSession,
        "refresh" => ChatCommand::Refresh,
        "help" => {
            println!(
                "/model <name>, /provider <id>, /effort <low|medium|high|none>, /max-tokens <n|none>, /steer <text>, /interrupt [reason], /pause, /resume, /refresh, /quit"
            );
            return Ok(true);
        }
        "model" | "provider" => {
            println!("{command} picker is a P3 TUI overlay; pass a value in plain mode");
            return Ok(true);
        }
        other => {
            println!("unknown slash command /{other}; try /help");
            return Ok(true);
        }
    };
    let events = engine.handle_command(chat_command).await?;
    renderer.render_events(&events);
    Ok(true)
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
