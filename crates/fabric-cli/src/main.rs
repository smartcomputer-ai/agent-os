use std::{collections::BTreeMap, io::Write};

use anyhow::Context;
use clap::{Args as ClapArgs, Parser, Subcommand};
use fabric_client::{FabricControllerClient, FabricHostClient};
use fabric_protocol::{
    CloseSignal, ControllerExecRequest, ControllerSignalSessionRequest, ExecEventKind, ExecStdin,
    FabricBytes, FabricSandboxTarget, FabricSessionSignal, FabricSessionTarget,
    FsApplyPatchRequest, FsEditFileRequest, FsFileWriteRequest, FsGlobRequest, FsGrepRequest,
    FsMkdirRequest, FsPathQuery, FsRemoveRequest, NetworkMode, QuiesceSignal, RequestId,
    ResourceLimits, ResumeSignal, SessionId, SessionLabelsPatchRequest, SessionOpenRequest,
    SessionSignal, SignalSessionRequest, TerminateRuntimeSignal,
};
use futures_util::StreamExt;

const DEFAULT_HOST_ENDPOINT: &str = "http://127.0.0.1:8791";
const DEFAULT_CONTROLLER_ENDPOINT: &str = "http://127.0.0.1:8788";

#[derive(Debug, Parser)]
#[command(name = "fabric", about = "Fabric development CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(alias = "h")]
    Host(HostArgs),
    #[command(alias = "c")]
    Controller(ControllerArgs),
}

#[derive(Debug, ClapArgs)]
struct HostArgs {
    #[arg(long, default_value = DEFAULT_HOST_ENDPOINT)]
    endpoint: String,

    #[command(subcommand)]
    command: HostCommand,
}

#[derive(Debug, ClapArgs)]
struct ControllerArgs {
    #[arg(long, default_value = DEFAULT_CONTROLLER_ENDPOINT)]
    endpoint: String,

    #[command(subcommand)]
    command: ControllerCommand,
}

#[derive(Debug, Subcommand)]
enum HostCommand {
    Health,
    Info,
    Inventory,
    Open {
        #[arg(long)]
        image: String,

        #[arg(long)]
        workdir: Option<String>,

        #[arg(long)]
        session_id: Option<String>,

        #[arg(long = "label")]
        labels: Vec<String>,

        #[arg(long)]
        net: bool,
    },
    Status {
        session_id: String,
    },
    Exec {
        session_id: String,

        #[arg(long)]
        cwd: Option<String>,

        #[arg(long)]
        timeout_secs: Option<u64>,

        #[arg(required = true, trailing_var_arg = true)]
        argv: Vec<String>,
    },
    Signal {
        session_id: String,
        action: String,
    },
    Fs {
        session_id: String,

        #[command(subcommand)]
        command: FsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ControllerCommand {
    Health,
    Info,
    Hosts,
    Host {
        host_id: String,
    },
    HostInventory {
        host_id: String,
    },
    Sessions {
        #[arg(long = "label")]
        labels: Vec<String>,
    },
    Open {
        #[arg(long)]
        image: String,

        #[arg(long)]
        workdir: Option<String>,

        #[arg(long)]
        request_id: Option<String>,

        #[arg(long = "label")]
        labels: Vec<String>,

        #[arg(long)]
        ttl_secs: Option<u64>,

        #[arg(long)]
        net: bool,
    },
    Status {
        session_id: String,
    },
    Labels {
        session_id: String,

        #[arg(long = "set")]
        set: Vec<String>,

        #[arg(long = "remove")]
        remove: Vec<String>,
    },
    Exec {
        session_id: String,

        #[arg(long)]
        request_id: Option<String>,

        #[arg(long)]
        cwd: Option<String>,

        #[arg(long)]
        timeout_secs: Option<u64>,

        #[arg(long)]
        stdin: Option<String>,

        #[arg(required = true, trailing_var_arg = true)]
        argv: Vec<String>,
    },
    Signal {
        session_id: String,
        action: String,
    },
    Fs {
        session_id: String,

        #[command(subcommand)]
        command: FsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum FsCommand {
    Read {
        path: String,

        #[arg(long)]
        offset_bytes: Option<u64>,

        #[arg(long)]
        max_bytes: Option<u64>,
    },
    Write {
        path: String,
        text: String,

        #[arg(long)]
        create_parents: bool,
    },
    Edit {
        path: String,
        old_string: String,
        new_string: String,

        #[arg(long)]
        replace_all: bool,
    },
    ApplyPatch {
        patch: String,

        #[arg(long, default_value = "v4a")]
        patch_format: String,

        #[arg(long)]
        dry_run: bool,
    },
    Mkdir {
        path: String,

        #[arg(long)]
        parents: bool,
    },
    Remove {
        path: String,

        #[arg(long)]
        recursive: bool,
    },
    Exists {
        path: String,
    },
    Stat {
        path: String,
    },
    List {
        path: Option<String>,
    },
    Grep {
        pattern: String,

        path: Option<String>,

        #[arg(long)]
        glob_filter: Option<String>,

        #[arg(long)]
        max_results: Option<u64>,

        #[arg(short = 'i', long)]
        case_insensitive: bool,
    },
    Glob {
        pattern: String,

        path: Option<String>,

        #[arg(long)]
        max_results: Option<u64>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Host(args) => handle_host(args).await,
        Command::Controller(args) => handle_controller(args).await,
    }
}

async fn handle_host(args: HostArgs) -> anyhow::Result<()> {
    let client = FabricHostClient::new(args.endpoint);

    match args.command {
        HostCommand::Health => {
            let health = client.health().await.context("query fabric host health")?;
            println!("{}", serde_json::to_string_pretty(&health)?);
        }
        HostCommand::Info => {
            let info = client.info().await.context("query fabric host info")?;
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        HostCommand::Inventory => {
            let inventory = client
                .inventory()
                .await
                .context("query fabric host inventory")?;
            println!("{}", serde_json::to_string_pretty(&inventory)?);
        }
        HostCommand::Open {
            image,
            workdir,
            session_id,
            labels,
            net,
        } => {
            let request = SessionOpenRequest {
                session_id: session_id.map(SessionId),
                image,
                runtime_class: Some("smolvm".to_owned()),
                workdir,
                env: BTreeMap::new(),
                network_mode: network_mode(net),
                mounts: Vec::new(),
                resources: ResourceLimits::default(),
                ttl_secs: None,
                labels: parse_labels(labels)?,
            };
            let response = client
                .open_session(&request)
                .await
                .context("open fabric host session")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        HostCommand::Status { session_id } => {
            let response = client
                .session_status(&SessionId(session_id))
                .await
                .context("query fabric host session status")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        HostCommand::Exec {
            session_id,
            cwd,
            timeout_secs,
            argv,
        } => {
            let request = fabric_protocol::ExecRequest {
                session_id: SessionId(session_id),
                argv,
                cwd,
                env: BTreeMap::new(),
                stdin: None,
                timeout_secs,
            };
            let events = client
                .exec_session_stream(&request)
                .await
                .context("exec fabric host session command")?;
            print_exec_stream(events).await?;
        }
        HostCommand::Signal { session_id, action } => {
            let request = SignalSessionRequest {
                action: parse_host_signal(&action)?,
            };
            let response = client
                .signal_session(&SessionId(session_id), &request)
                .await
                .context("signal fabric host session")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        HostCommand::Fs {
            session_id,
            command,
        } => {
            handle_fs_command(
                WorkspaceClient::Host(&client),
                SessionId(session_id),
                command,
            )
            .await?;
        }
    }

    Ok(())
}

async fn handle_controller(args: ControllerArgs) -> anyhow::Result<()> {
    let client = FabricControllerClient::new(args.endpoint);

    match args.command {
        ControllerCommand::Health => {
            let health = client
                .health()
                .await
                .context("query fabric controller health")?;
            println!("{}", serde_json::to_string_pretty(&health)?);
        }
        ControllerCommand::Info => {
            let info = client
                .info()
                .await
                .context("query fabric controller info")?;
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        ControllerCommand::Hosts => {
            let hosts = client
                .list_hosts()
                .await
                .context("query fabric controller hosts")?;
            println!("{}", serde_json::to_string_pretty(&hosts)?);
        }
        ControllerCommand::Host { host_id } => {
            let host = client
                .host(&fabric_protocol::HostId(host_id))
                .await
                .context("query fabric controller host")?;
            println!("{}", serde_json::to_string_pretty(&host)?);
        }
        ControllerCommand::HostInventory { host_id } => {
            let inventory = client
                .host_inventory(&fabric_protocol::HostId(host_id))
                .await
                .context("query fabric controller host inventory")?;
            println!("{}", serde_json::to_string_pretty(&inventory)?);
        }
        ControllerCommand::Sessions { labels } => {
            let sessions = client
                .list_sessions(&parse_label_filters(labels)?)
                .await
                .context("query fabric controller sessions")?;
            println!("{}", serde_json::to_string_pretty(&sessions)?);
        }
        ControllerCommand::Open {
            image,
            workdir,
            request_id,
            labels,
            ttl_secs,
            net,
        } => {
            let request = fabric_protocol::ControllerSessionOpenRequest {
                request_id: request_id.map(RequestId),
                target: FabricSessionTarget::Sandbox(FabricSandboxTarget {
                    image,
                    runtime_class: Some("smolvm".to_owned()),
                    workdir,
                    env: BTreeMap::new(),
                    network_mode: network_mode(net),
                    mounts: Vec::new(),
                    resources: ResourceLimits::default(),
                }),
                ttl_ns: ttl_secs.map(secs_to_ns),
                labels: parse_labels(labels)?,
            };
            let response = client
                .open_session(&request)
                .await
                .context("open fabric controller session")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        ControllerCommand::Status { session_id } => {
            let response = client
                .session(&SessionId(session_id))
                .await
                .context("query fabric controller session status")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        ControllerCommand::Labels {
            session_id,
            set,
            remove,
        } => {
            let request = SessionLabelsPatchRequest {
                set: parse_labels(set)?,
                remove,
            };
            let response = client
                .patch_session_labels(&SessionId(session_id), &request)
                .await
                .context("patch fabric controller session labels")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        ControllerCommand::Exec {
            session_id,
            request_id,
            cwd,
            timeout_secs,
            stdin,
            argv,
        } => {
            let request = ControllerExecRequest {
                request_id: request_id.map(RequestId),
                argv,
                cwd,
                env_patch: BTreeMap::new(),
                stdin: stdin.map(ExecStdin::Text),
                timeout_ns: timeout_secs.map(secs_to_ns),
            };
            let events = client
                .exec_session_stream(&SessionId(session_id), &request)
                .await
                .context("exec fabric controller session command")?;
            print_exec_stream(events).await?;
        }
        ControllerCommand::Signal { session_id, action } => {
            let request = ControllerSignalSessionRequest {
                request_id: None,
                signal: parse_controller_signal(&action)?,
            };
            let response = client
                .signal_session(&SessionId(session_id), &request)
                .await
                .context("signal fabric controller session")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        ControllerCommand::Fs {
            session_id,
            command,
        } => {
            handle_fs_command(
                WorkspaceClient::Controller(&client),
                SessionId(session_id),
                command,
            )
            .await?;
        }
    }

    Ok(())
}

enum WorkspaceClient<'a> {
    Host(&'a FabricHostClient),
    Controller(&'a FabricControllerClient),
}

impl WorkspaceClient<'_> {
    async fn read_file(
        &self,
        session_id: &SessionId,
        query: &FsPathQuery,
    ) -> Result<fabric_protocol::FsFileReadResponse, fabric_client::FabricClientError> {
        match self {
            Self::Host(client) => client.read_file(session_id, query).await,
            Self::Controller(client) => client.read_file(session_id, query).await,
        }
    }

    async fn write_file(
        &self,
        session_id: &SessionId,
        request: &FsFileWriteRequest,
    ) -> Result<fabric_protocol::FsWriteResponse, fabric_client::FabricClientError> {
        match self {
            Self::Host(client) => client.write_file(session_id, request).await,
            Self::Controller(client) => client.write_file(session_id, request).await,
        }
    }

    async fn edit_file(
        &self,
        session_id: &SessionId,
        request: &FsEditFileRequest,
    ) -> Result<fabric_protocol::FsEditFileResponse, fabric_client::FabricClientError> {
        match self {
            Self::Host(client) => client.edit_file(session_id, request).await,
            Self::Controller(client) => client.edit_file(session_id, request).await,
        }
    }

    async fn apply_patch(
        &self,
        session_id: &SessionId,
        request: &FsApplyPatchRequest,
    ) -> Result<fabric_protocol::FsApplyPatchResponse, fabric_client::FabricClientError> {
        match self {
            Self::Host(client) => client.apply_patch(session_id, request).await,
            Self::Controller(client) => client.apply_patch(session_id, request).await,
        }
    }

    async fn mkdir(
        &self,
        session_id: &SessionId,
        request: &FsMkdirRequest,
    ) -> Result<fabric_protocol::FsStatResponse, fabric_client::FabricClientError> {
        match self {
            Self::Host(client) => client.mkdir(session_id, request).await,
            Self::Controller(client) => client.mkdir(session_id, request).await,
        }
    }

    async fn remove(
        &self,
        session_id: &SessionId,
        request: &FsRemoveRequest,
    ) -> Result<fabric_protocol::FsRemoveResponse, fabric_client::FabricClientError> {
        match self {
            Self::Host(client) => client.remove(session_id, request).await,
            Self::Controller(client) => client.remove(session_id, request).await,
        }
    }

    async fn exists(
        &self,
        session_id: &SessionId,
        query: &FsPathQuery,
    ) -> Result<fabric_protocol::FsExistsResponse, fabric_client::FabricClientError> {
        match self {
            Self::Host(client) => client.exists(session_id, query).await,
            Self::Controller(client) => client.exists(session_id, query).await,
        }
    }

    async fn stat(
        &self,
        session_id: &SessionId,
        query: &FsPathQuery,
    ) -> Result<fabric_protocol::FsStatResponse, fabric_client::FabricClientError> {
        match self {
            Self::Host(client) => client.stat(session_id, query).await,
            Self::Controller(client) => client.stat(session_id, query).await,
        }
    }

    async fn list_dir(
        &self,
        session_id: &SessionId,
        query: &FsPathQuery,
    ) -> Result<fabric_protocol::FsListDirResponse, fabric_client::FabricClientError> {
        match self {
            Self::Host(client) => client.list_dir(session_id, query).await,
            Self::Controller(client) => client.list_dir(session_id, query).await,
        }
    }

    async fn grep(
        &self,
        session_id: &SessionId,
        request: &FsGrepRequest,
    ) -> Result<fabric_protocol::FsGrepResponse, fabric_client::FabricClientError> {
        match self {
            Self::Host(client) => client.grep(session_id, request).await,
            Self::Controller(client) => client.grep(session_id, request).await,
        }
    }

    async fn glob(
        &self,
        session_id: &SessionId,
        request: &FsGlobRequest,
    ) -> Result<fabric_protocol::FsGlobResponse, fabric_client::FabricClientError> {
        match self {
            Self::Host(client) => client.glob(session_id, request).await,
            Self::Controller(client) => client.glob(session_id, request).await,
        }
    }
}

async fn handle_fs_command(
    client: WorkspaceClient<'_>,
    session_id: SessionId,
    command: FsCommand,
) -> anyhow::Result<()> {
    match command {
        FsCommand::Read {
            path,
            offset_bytes,
            max_bytes,
        } => {
            let response = client
                .read_file(
                    &session_id,
                    &FsPathQuery {
                        path,
                        offset_bytes,
                        max_bytes,
                    },
                )
                .await
                .context("read fabric workspace file")?;
            let bytes = response
                .content
                .decode_bytes()
                .map_err(anyhow::Error::msg)?;
            std::io::stdout().write_all(&bytes)?;
        }
        FsCommand::Write {
            path,
            text,
            create_parents,
        } => {
            let response = client
                .write_file(
                    &session_id,
                    &FsFileWriteRequest {
                        path,
                        content: FabricBytes::Text(text),
                        create_parents,
                    },
                )
                .await
                .context("write fabric workspace file")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        FsCommand::Edit {
            path,
            old_string,
            new_string,
            replace_all,
        } => {
            let response = client
                .edit_file(
                    &session_id,
                    &FsEditFileRequest {
                        path,
                        old_string,
                        new_string,
                        replace_all,
                    },
                )
                .await
                .context("edit fabric workspace file")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        FsCommand::ApplyPatch {
            patch,
            patch_format,
            dry_run,
        } => {
            let response = client
                .apply_patch(
                    &session_id,
                    &FsApplyPatchRequest {
                        patch,
                        patch_format: Some(patch_format),
                        dry_run,
                    },
                )
                .await
                .context("apply fabric workspace patch")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        FsCommand::Mkdir { path, parents } => {
            let response = client
                .mkdir(&session_id, &FsMkdirRequest { path, parents })
                .await
                .context("create fabric workspace directory")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        FsCommand::Remove { path, recursive } => {
            let response = client
                .remove(&session_id, &FsRemoveRequest { path, recursive })
                .await
                .context("remove fabric workspace path")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        FsCommand::Exists { path } => {
            let response = client
                .exists(&session_id, &path_query(path))
                .await
                .context("check fabric workspace path")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        FsCommand::Stat { path } => {
            let response = client
                .stat(&session_id, &path_query(path))
                .await
                .context("stat fabric workspace path")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        FsCommand::List { path } => {
            let response = client
                .list_dir(
                    &session_id,
                    &path_query(path.unwrap_or_else(|| ".".to_owned())),
                )
                .await
                .context("list fabric workspace directory")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        FsCommand::Grep {
            pattern,
            path,
            glob_filter,
            max_results,
            case_insensitive,
        } => {
            let response = client
                .grep(
                    &session_id,
                    &FsGrepRequest {
                        pattern,
                        path,
                        glob_filter,
                        max_results,
                        case_insensitive,
                    },
                )
                .await
                .context("grep fabric workspace")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        FsCommand::Glob {
            pattern,
            path,
            max_results,
        } => {
            let response = client
                .glob(
                    &session_id,
                    &FsGlobRequest {
                        pattern,
                        path,
                        max_results,
                    },
                )
                .await
                .context("glob fabric workspace")?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
    }
    Ok(())
}

async fn print_exec_stream(mut events: fabric_client::ExecEventClientStream) -> anyhow::Result<()> {
    let mut exit_code = 0;
    while let Some(event) = events.next().await {
        let event = event.context("read fabric exec event")?;
        match event.kind {
            ExecEventKind::Started => {}
            ExecEventKind::Stdout => {
                if let Some(data) = event.data {
                    let bytes = data.decode_bytes().map_err(anyhow::Error::msg)?;
                    std::io::stdout().write_all(&bytes)?;
                }
            }
            ExecEventKind::Stderr => {
                if let Some(data) = event.data {
                    let bytes = data.decode_bytes().map_err(anyhow::Error::msg)?;
                    std::io::stderr().write_all(&bytes)?;
                }
            }
            ExecEventKind::Exit => {
                exit_code = event.exit_code.unwrap_or_default();
            }
            ExecEventKind::Error => {
                if let Some(message) = event.message.or_else(|| {
                    event
                        .data
                        .as_ref()
                        .and_then(|data| data.as_text())
                        .map(str::to_owned)
                }) {
                    eprintln!("{message}");
                }
                exit_code = 1;
            }
        }
    }
    std::process::exit(exit_code);
}

fn path_query(path: String) -> FsPathQuery {
    FsPathQuery {
        path,
        offset_bytes: None,
        max_bytes: None,
    }
}

fn network_mode(net: bool) -> NetworkMode {
    if net {
        NetworkMode::Egress
    } else {
        NetworkMode::Disabled
    }
}

fn secs_to_ns(secs: u64) -> u128 {
    u128::from(secs) * 1_000_000_000
}

fn parse_labels(values: Vec<String>) -> anyhow::Result<BTreeMap<String, String>> {
    values
        .into_iter()
        .map(|value| {
            let (key, label_value) = parse_key_value(&value)?;
            Ok((key.to_owned(), label_value.to_owned()))
        })
        .collect()
}

fn parse_label_filters(values: Vec<String>) -> anyhow::Result<Vec<(String, String)>> {
    values
        .into_iter()
        .map(|value| {
            let (key, label_value) = parse_key_value(&value)?;
            Ok((key.to_owned(), label_value.to_owned()))
        })
        .collect()
}

fn parse_key_value(value: &str) -> anyhow::Result<(&str, &str)> {
    let Some((key, parsed_value)) = value.split_once('=') else {
        anyhow::bail!("expected key=value, got '{value}'");
    };
    if key.is_empty() {
        anyhow::bail!("label key must not be empty");
    }
    Ok((key, parsed_value))
}

fn parse_host_signal(value: &str) -> anyhow::Result<SessionSignal> {
    match value {
        "quiesce" | "stop" => Ok(SessionSignal::Quiesce),
        "resume" | "start" => Ok(SessionSignal::Resume),
        "terminate" | "kill" => Ok(SessionSignal::Terminate),
        "close" | "delete" => Ok(SessionSignal::Close),
        _ => anyhow::bail!(
            "unsupported host signal '{value}', expected quiesce, resume, terminate, or close"
        ),
    }
}

fn parse_controller_signal(value: &str) -> anyhow::Result<FabricSessionSignal> {
    match value {
        "quiesce" | "stop" => Ok(FabricSessionSignal::Quiesce(QuiesceSignal {})),
        "resume" | "start" => Ok(FabricSessionSignal::Resume(ResumeSignal {})),
        "close" | "delete" => Ok(FabricSessionSignal::Close(CloseSignal {})),
        "terminate-runtime" | "terminate" | "kill" => Ok(FabricSessionSignal::TerminateRuntime(
            TerminateRuntimeSignal {},
        )),
        _ => anyhow::bail!(
            "unsupported controller signal '{value}', expected quiesce, resume, close, or terminate-runtime"
        ),
    }
}
