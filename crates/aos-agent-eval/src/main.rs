mod casefile;
mod eval_host;

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail, ensure};
use aos_agent::{
    HostSessionStatus, SessionConfig, SessionId, SessionIngress, SessionIngressKind,
    SessionLifecycle, SessionState, default_tool_profile_for_provider, default_tool_profiles,
    default_tool_registry,
};
use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effect_adapters::adapters::llm::LlmAdapter;
use aos_effect_adapters::config::{
    EffectAdapterConfig, LlmAdapterConfig, LlmApiKind, ProviderConfig,
};
use aos_effect_adapters::traits::AsyncEffectAdapter;
use aos_effects::builtins::{
    HostLocalTarget, HostSessionOpenParams, HostSessionOpenReceipt, HostTarget, LlmGenerateParams,
    LlmGenerateReceipt,
};
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use aos_kernel::Store;
use aos_node::WorldConfig;
use casefile::{EvalCase, FileExpectation, load_cases};
use clap::{Parser, Subcommand, ValueEnum};
use eval_host::{EvalHost, EvalHostConfig, EvalModuleBuild, EvalModulePatch};
use serde_json::{Value, json};
use tempfile::TempDir;

const WORKFLOW_NAME: &str = "aos.agent/SessionWorkflow@1";
const DIRECT_EVENT_SCHEMA: &str = "aos.agent/SessionIngress@1";
const EVAL_ASSETS_ROOT: &str = "crates/aos-agent-eval/fixtures/eval-world/air";
const CASES_ROOT: &str = "crates/aos-agent-eval/cases";
const SDK_AIR_ROOT: &str = "crates/aos-agent/air";
const SDK_WASM_PACKAGE: &str = "aos-agent";
const SDK_WASM_BIN: &str = "session_workflow";

const OPENAI_KEY_ENV: &str = "OPENAI_API_KEY";
const OPENAI_BASE_URL_ENV: &str = "OPENAI_BASE_URL";

const ANTHROPIC_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const ANTHROPIC_MODEL_ENV: &str = "ANTHROPIC_LIVE_MODEL";
const ANTHROPIC_BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";

#[derive(Parser, Debug)]
#[command(
    name = "aos-agent-eval",
    version,
    about = "Run prompt-level agent tool evals"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, global = true, value_enum, default_value_t = ProviderChoice::Openai)]
    provider: ProviderChoice,

    #[arg(
        long,
        global = true,
        help = "Override provider model for this invocation"
    )]
    model: Option<String>,

    #[arg(long, global = true, help = "Override number of runs per case")]
    runs: Option<u32>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// List available eval cases
    List,
    /// Run a single case by id
    Case { id: String },
    /// Run all cases
    All,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ProviderChoice {
    Openai,
    Anthropic,
}

#[derive(Debug, Clone)]
struct ProviderRuntime {
    provider_id: String,
    api_kind: LlmApiKind,
    api_key: String,
    model: String,
    base_url: String,
}

#[derive(Debug, Clone)]
struct CaseRunSummary {
    case_id: String,
    passed_runs: u32,
    total_runs: u32,
    min_pass_rate: f64,
}

#[derive(Debug, Clone)]
struct AttemptOutcome {
    passed: bool,
    failures: Vec<String>,
    used_tools: BTreeSet<String>,
    tool_arguments: Vec<String>,
    assistant_text: String,
    tool_outputs_text: String,
}

#[derive(Debug, Default)]
struct DriveStats {
    llm_turns: u32,
    effect_rounds: u32,
}

#[derive(Debug, Default)]
struct ConversationObservations {
    assistant_text: String,
    tool_names: BTreeSet<String>,
    tool_outputs: Vec<String>,
    tool_arguments: Vec<String>,
}

struct EvalInvocation {
    _world_temp: TempDir,
    workspaces_root: PathBuf,
    host: EvalHost,
    clock: u64,
    next_session_counter: u64,
}

impl EvalInvocation {
    fn new() -> Result<Self> {
        let world_temp = TempDir::new().context("create eval world tempdir")?;
        let world_root = world_temp.path().to_path_buf();
        let workspaces_root = world_root.join("workspaces");
        fs::create_dir_all(&workspaces_root)
            .with_context(|| format!("create workspaces root {}", workspaces_root.display()))?;

        let assets_root = workspace_root().join(EVAL_ASSETS_ROOT);
        let sdk_air_root = workspace_root().join(SDK_AIR_ROOT);
        let import_roots = vec![sdk_air_root];
        let module_patches = vec![EvalModulePatch {
            module_name: WORKFLOW_NAME,
            build: EvalModuleBuild::CargoBin {
                package: SDK_WASM_PACKAGE,
                bin: SDK_WASM_BIN,
            },
        }];
        let host = EvalHost::prepare(EvalHostConfig {
            world_root: &world_root,
            assets_root: &assets_root,
            import_roots: &import_roots,
            workspace_root: workspace_root().as_path(),
            workflow_name: WORKFLOW_NAME,
            event_schema: DIRECT_EVENT_SCHEMA,
            world_config: WorldConfig::default(),
            adapter_config: EffectAdapterConfig {
                llm: None,
                ..EffectAdapterConfig::default()
            },
            module_patches: &module_patches,
        })?;

        Ok(Self {
            _world_temp: world_temp,
            workspaces_root,
            host,
            clock: 0,
            next_session_counter: 0,
        })
    }

    fn allocate_attempt(
        &mut self,
        case_id: &str,
        attempt_index: u32,
    ) -> Result<(SessionId, PathBuf)> {
        self.next_session_counter = self.next_session_counter.saturating_add(1);
        let session_id = SessionId(format!(
            "44444444-4444-4444-4444-{:012x}",
            self.next_session_counter
        ));
        let workdir = self.workspaces_root.join(format!(
            "{}-run-{}-{}",
            sanitize_case_id(case_id),
            attempt_index,
            self.next_session_counter
        ));
        fs::create_dir_all(&workdir)
            .with_context(|| format!("create workdir {}", workdir.display()))?;
        Ok((session_id, workdir))
    }
}

fn main() {
    if let Err(err) = run_cli() {
        eprintln!("error: {err}");
        for cause in err.chain().skip(1) {
            eprintln!("  caused by: {cause}");
        }
        std::process::exit(1);
    }
}

fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    let cases_dir = workspace_root().join(CASES_ROOT);
    let cases = load_cases(&cases_dir)?;

    match cli.command {
        Commands::List => {
            for case in &cases {
                println!("{:<20} {}", case.id, case.description);
            }
            return Ok(());
        }
        Commands::Case { id } => {
            let provider = resolve_provider(cli.provider, cli.model.clone())?;
            let case = cases
                .into_iter()
                .find(|c| c.id == id)
                .ok_or_else(|| anyhow!("unknown case '{id}'"))?;
            let mut invocation = EvalInvocation::new()?;
            let summary = run_case(&mut invocation, &case, &provider, cli.runs)?;
            print_case_summary(&summary);
            let pass_rate = summary.passed_runs as f64 / summary.total_runs as f64;
            if pass_rate + f64::EPSILON < summary.min_pass_rate {
                bail!(
                    "case '{}' below threshold: {:.2} < {:.2}",
                    summary.case_id,
                    pass_rate,
                    summary.min_pass_rate
                );
            }
            return Ok(());
        }
        Commands::All => {}
    }

    let provider = resolve_provider(cli.provider, cli.model.clone())?;
    let mut invocation = EvalInvocation::new()?;

    let mut failures = Vec::new();
    let mut summaries = Vec::new();

    for case in &cases {
        let summary = run_case(&mut invocation, case, &provider, cli.runs)?;
        print_case_summary(&summary);
        let pass_rate = summary.passed_runs as f64 / summary.total_runs as f64;
        if pass_rate + f64::EPSILON < summary.min_pass_rate {
            failures.push(format!(
                "{} ({:.2} < {:.2})",
                summary.case_id, pass_rate, summary.min_pass_rate
            ));
        }
        summaries.push(summary);
    }

    let total_runs: u32 = summaries.iter().map(|s| s.total_runs).sum();
    let passed_runs: u32 = summaries.iter().map(|s| s.passed_runs).sum();
    let aggregate = if total_runs == 0 {
        0.0
    } else {
        passed_runs as f64 / total_runs as f64
    };
    println!(
        "\nAggregate: passed_runs={}/{} ({:.2})",
        passed_runs, total_runs, aggregate
    );

    if !failures.is_empty() {
        bail!("case thresholds failed: {}", failures.join(", "));
    }

    Ok(())
}

fn run_case(
    invocation: &mut EvalInvocation,
    case: &EvalCase,
    provider: &ProviderRuntime,
    runs_override: Option<u32>,
) -> Result<CaseRunSummary> {
    let runs = runs_override.or(case.eval.runs).unwrap_or(1).max(1);
    let min_pass_rate = case.eval.min_pass_rate.unwrap_or(1.0).clamp(0.0, 1.0);

    println!("\nCase file: {}", case.source_file);
    println!("\nCase {}: {}", case.id, case.description);
    let mut passed_runs = 0_u32;

    for attempt in 0..runs {
        let outcome = run_attempt(invocation, case, provider, attempt + 1)?;
        if outcome.passed {
            passed_runs = passed_runs.saturating_add(1);
            println!(
                "  run {:>2}: PASS tools={:?}",
                attempt + 1,
                outcome.used_tools
            );
        } else {
            println!("  run {:>2}: FAIL", attempt + 1);
            for failure in &outcome.failures {
                println!("    - {failure}");
            }
            if !outcome.used_tools.is_empty() {
                println!("    tools={:?}", outcome.used_tools);
            }
            if !outcome.tool_arguments.is_empty() {
                println!("    tool_args={:?}", outcome.tool_arguments);
            }
            if !outcome.assistant_text.is_empty() {
                println!("    assistant={}", preview(&outcome.assistant_text));
            }
            if !outcome.tool_outputs_text.is_empty() {
                println!("    tool_output={}", preview(&outcome.tool_outputs_text));
            }
        }
    }

    Ok(CaseRunSummary {
        case_id: case.id.clone(),
        passed_runs,
        total_runs: runs,
        min_pass_rate,
    })
}

fn print_case_summary(summary: &CaseRunSummary) {
    let pass_rate = if summary.total_runs == 0 {
        0.0
    } else {
        summary.passed_runs as f64 / summary.total_runs as f64
    };
    println!(
        "  summary: {}/{} pass ({:.2}), threshold={:.2}",
        summary.passed_runs, summary.total_runs, pass_rate, summary.min_pass_rate
    );
}

fn run_attempt(
    invocation: &mut EvalInvocation,
    case: &EvalCase,
    provider: &ProviderRuntime,
    attempt_index: u32,
) -> Result<AttemptOutcome> {
    let (session_id, workdir) = invocation.allocate_attempt(&case.id, attempt_index)?;
    seed_files(case, &workdir)?;

    let default_profile_id = case
        .run
        .tool_profile
        .clone()
        .unwrap_or_else(|| default_tool_profile_for_provider(&provider.provider_id));

    install_tool_registry(
        &mut invocation.host,
        &mut invocation.clock,
        &session_id,
        &default_profile_id,
        case.run.allowed_tools.as_deref(),
    )?;

    if case.run.bootstrap_session.unwrap_or(true) {
        let host_session_id = bootstrap_host_session(&mut invocation.host, &workdir)?;
        send_session_event(
            &mut invocation.host,
            &mut invocation.clock,
            &session_id,
            SessionIngressKind::HostSessionUpdated {
                host_session_id: Some(host_session_id),
                host_session_status: Some(HostSessionStatus::Ready),
            },
        )?;
    }

    let input_ref = store_json_blob(
        invocation.host.store().as_ref(),
        &json!({
            "role": "user",
            "content": case.prompt,
        }),
    )?;

    send_session_event(
        &mut invocation.host,
        &mut invocation.clock,
        &session_id,
        SessionIngressKind::RunRequested {
            input_ref: input_ref.as_str().to_string(),
            run_overrides: Some(SessionConfig {
                provider: provider.provider_id.clone(),
                model: provider.model.clone(),
                reasoning_effort: None,
                max_tokens: case.run.max_tokens,
                default_prompt_refs: None,
                default_tool_profile: Some(default_profile_id),
                default_tool_enable: case.run.tool_enable.clone(),
                default_tool_disable: case.run.tool_disable.clone(),
                default_tool_force: case.run.tool_force.clone(),
            }),
        },
    )?;

    let llm_adapter = make_adapter(invocation.host.store(), provider);
    let max_steps = case.eval.max_steps.unwrap_or(96).max(1);
    let stats = drive_live_effects(
        &mut invocation.host,
        &llm_adapter,
        &provider.api_key,
        max_steps,
    )?;

    let state: SessionState = invocation.host.read_state_for_session(&session_id.0)?;
    let observations = collect_conversation_observations(invocation.host.store().as_ref(), &state);
    let assistant_text = observations.assistant_text;
    let mut used_tools = observations.tool_names;
    let mut tool_outputs = observations.tool_outputs;
    let mut tool_arguments = observations.tool_arguments;
    if let Some(batch) = state.active_tool_batch.as_ref() {
        for call in &batch.plan.observed_calls {
            used_tools.insert(call.tool_name.clone());
        }
        for call in &batch.plan.planned_calls {
            if !call.accepted {
                continue;
            }
            tool_arguments.push(format!("{} {}", call.tool_name, call.arguments_json));
        }
        for result in batch.llm_results.values() {
            tool_outputs.push(result.output_json.clone());
        }
    }
    let tool_outputs_text = tool_outputs.join("\n");
    let used_tool_ids = resolve_tool_ids(&state, &used_tools);

    let mut failures = Vec::new();
    if !matches!(
        state.lifecycle,
        SessionLifecycle::WaitingInput | SessionLifecycle::Completed
    ) {
        failures.push(format!(
            "unexpected lifecycle {:?} after {} rounds",
            state.lifecycle, stats.effect_rounds
        ));
    }

    for tool in &case.expect.tool_called {
        if !used_tool_ids.contains(tool) {
            failures.push(format!(
                "expected tool call '{}', observed {:?}",
                tool, used_tool_ids
            ));
        }
    }

    let assistant_lower = assistant_text.to_ascii_lowercase();
    for needle in &case.expect.assistant_contains {
        if !assistant_lower.contains(&needle.to_ascii_lowercase()) {
            failures.push(format!(
                "assistant text missing '{}' (assistant={})",
                needle,
                preview(&assistant_text)
            ));
        }
    }

    let tool_outputs_lower = tool_outputs_text.to_ascii_lowercase();
    for needle in &case.expect.tool_output_contains {
        if !tool_outputs_lower.contains(&needle.to_ascii_lowercase()) {
            failures.push(format!(
                "tool output missing '{}' (tool_output={})",
                needle,
                preview(&tool_outputs_text)
            ));
        }
    }

    for file_expectation in &case.expect.files {
        validate_file_expectation(&workdir, file_expectation, &mut failures)?;
    }

    Ok(AttemptOutcome {
        passed: failures.is_empty(),
        failures,
        used_tools: used_tool_ids,
        tool_arguments,
        assistant_text,
        tool_outputs_text,
    })
}

fn seed_files(case: &EvalCase, workdir: &Path) -> Result<()> {
    for file in &case.setup.files {
        let path = workdir.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create parent dirs for {}", path.display()))?;
        }
        fs::write(&path, file.content.as_bytes())
            .with_context(|| format!("seed file {}", path.display()))?;
    }
    Ok(())
}

fn validate_file_expectation(
    workdir: &Path,
    expectation: &FileExpectation,
    failures: &mut Vec<String>,
) -> Result<()> {
    let path = workdir.join(&expectation.path);
    let exists = path.exists();

    if let Some(expected_exists) = expectation.exists
        && expected_exists != exists
    {
        failures.push(format!(
            "file '{}' existence mismatch: expected {}, got {}",
            expectation.path, expected_exists, exists
        ));
    }

    if expectation.equals.is_none() && expectation.contains.is_none() {
        return Ok(());
    }

    if !exists {
        failures.push(format!(
            "expected file '{}' for content assertion, but it does not exist",
            expectation.path
        ));
        return Ok(());
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("read assertion file {}", path.display()))?;

    if let Some(expected) = expectation.equals.as_ref()
        && &content != expected
    {
        failures.push(format!(
            "file '{}' mismatch: expected {:?}, got {:?}",
            expectation.path, expected, content
        ));
    }

    if let Some(needle) = expectation.contains.as_ref()
        && !content.contains(needle)
    {
        failures.push(format!(
            "file '{}' missing substring {:?}; content={:?}",
            expectation.path, needle, content
        ));
    }

    Ok(())
}

fn send_session_event(
    host: &mut EvalHost,
    clock: &mut u64,
    session_id: &SessionId,
    kind: SessionIngressKind,
) -> Result<()> {
    *clock = clock.saturating_add(1);
    host.send_event(&SessionIngress {
        session_id: session_id.clone(),
        observed_at_ns: *clock,
        ingress: kind,
    })
}

fn install_tool_registry(
    host: &mut EvalHost,
    clock: &mut u64,
    session_id: &SessionId,
    default_profile: &str,
    allowed_tools: Option<&[String]>,
) -> Result<()> {
    let registry = default_tool_registry();
    let mut profiles = default_tool_profiles();

    if let Some(allowed) = allowed_tools {
        if allowed.is_empty() {
            bail!("allowed_tools is empty for profile '{}'", default_profile);
        }
        for tool_id in allowed {
            if !registry.contains_key(tool_id) {
                bail!("unknown tool id '{}' in allowed_tools", tool_id);
            }
        }
        profiles.insert(default_profile.to_string(), allowed.to_vec());
    } else {
        ensure!(
            profiles.contains_key(default_profile),
            "unknown default tool profile '{}'",
            default_profile
        );
    }

    send_session_event(
        host,
        clock,
        session_id,
        SessionIngressKind::ToolRegistrySet {
            registry,
            profiles: Some(profiles),
            default_profile: Some(default_profile.to_string()),
        },
    )?;

    Ok(())
}

fn resolve_tool_ids(state: &SessionState, llm_tool_names: &BTreeSet<String>) -> BTreeSet<String> {
    let mut llm_to_id = HashMap::new();
    for (tool_id, spec) in &state.tool_registry {
        llm_to_id.insert(spec.tool_name.clone(), tool_id.clone());
    }
    llm_tool_names
        .iter()
        .map(|tool_name| {
            llm_to_id
                .get(tool_name)
                .cloned()
                .unwrap_or_else(|| tool_name.clone())
        })
        .collect::<BTreeSet<_>>()
}

fn bootstrap_host_session(host: &mut EvalHost, workdir: &Path) -> Result<String> {
    let params = HostSessionOpenParams {
        target: HostTarget::local(HostLocalTarget {
            mounts: None,
            workdir: Some(workdir.to_string_lossy().to_string()),
            env: None,
            network_mode: "none".into(),
        }),
        session_ttl_ns: None,
        labels: None,
    };

    let intent = EffectIntent::from_raw_params(
        EffectKind::host_session_open(),
        serde_cbor::to_vec(&params).context("encode host.session.open params")?,
        [0x11; 32],
    )
    .context("build host.session.open intent")?;

    let receipts =
        host.execute_batch_routed(vec![(intent, EffectKind::HOST_SESSION_OPEN.to_string())])?;
    let receipt = receipts
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("missing host.session.open receipt"))?;

    ensure!(
        receipt.status == ReceiptStatus::Ok,
        "host.session.open failed with status {:?}",
        receipt.status
    );

    let payload: HostSessionOpenReceipt = serde_cbor::from_slice(&receipt.payload_cbor)
        .context("decode host.session.open receipt")?;
    ensure!(
        payload.status == "ready",
        "host.session.open returned status {}",
        payload.status
    );
    Ok(payload.session_id)
}

fn drive_live_effects<S: Store + 'static>(
    host: &mut EvalHost,
    llm_adapter: &LlmAdapter<S>,
    api_key: &str,
    max_steps: u32,
) -> Result<DriveStats> {
    let mut stats = DriveStats::default();

    for _ in 0..max_steps {
        host.run_to_idle()?;
        let intents = host.with_kernel_mut(|kernel| kernel.drain_effects())?;
        if intents.is_empty() {
            return Ok(stats);
        }

        stats.effect_rounds = stats.effect_rounds.saturating_add(1);

        let mut receipts = Vec::<EffectReceipt>::new();
        let mut external = Vec::<(EffectIntent, String)>::new();

        for intent in intents {
            if let Some(internal_receipt) =
                host.with_kernel_mut(|kernel| kernel.handle_internal_intent(&intent))?
            {
                receipts.push(internal_receipt);
                continue;
            }

            if intent.kind.as_str() == EffectKind::LLM_GENERATE {
                stats.llm_turns = stats.llm_turns.saturating_add(1);
                let receipt = execute_live_llm_intent(host, llm_adapter, intent, api_key)?;
                receipts.push(receipt);
                continue;
            }

            if std::env::var("AOS_AGENT_EVAL_DEBUG_PATCH").is_ok()
                && intent.kind.as_str() == EffectKind::HOST_FS_APPLY_PATCH
            {
                if let Ok(value) = serde_cbor::from_slice::<Value>(&intent.params_cbor) {
                    eprintln!("debug host.fs.apply_patch params: {}", value);
                }
            }

            external.push((intent.clone(), intent.kind.as_str().to_string()));
        }

        if !external.is_empty() {
            let external_receipts = host.execute_batch_routed(external)?;
            receipts.extend(external_receipts);
        }

        for receipt in receipts {
            host.with_kernel_mut(|kernel| kernel.handle_receipt(receipt))?;
        }
    }

    bail!("eval safety trip: exceeded max dispatch rounds ({max_steps}) without quiescence")
}

fn execute_live_llm_intent<S: Store + 'static>(
    host: &EvalHost,
    llm_adapter: &LlmAdapter<S>,
    intent: EffectIntent,
    api_key: &str,
) -> Result<EffectReceipt> {
    let mut params: LlmGenerateParams =
        serde_cbor::from_slice(&intent.params_cbor).context("decode llm.generate params")?;
    params.api_key = Some(api_key.to_string().into());

    let patched_intent = EffectIntent::from_raw_params(
        EffectKind::llm_generate(),
        serde_cbor::to_vec(&params).context("encode patched llm.generate params")?,
        intent.idempotency_key,
    )
    .context("build patched llm intent")?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build eval llm runtime")?;
    let mut receipt = runtime
        .block_on(llm_adapter.run_terminal(&patched_intent))
        .context("execute live llm adapter")?;

    if receipt.status != ReceiptStatus::Ok {
        if let Ok(payload) = serde_cbor::from_slice::<LlmGenerateReceipt>(&receipt.payload_cbor) {
            if let Ok(message) = load_text_blob(host.store().as_ref(), payload.output_ref.as_str())
            {
                eprintln!(
                    "llm.generate failed: status={:?} detail={}",
                    receipt.status, message
                );
            } else {
                eprintln!(
                    "llm.generate failed: status={:?} (unable to decode error text)",
                    receipt.status
                );
            }
        } else {
            eprintln!(
                "llm.generate failed: status={:?} (invalid receipt payload)",
                receipt.status
            );
        }
    }

    // Kernel pending intent matching is based on the original emitted intent hash.
    receipt.intent_hash = intent.intent_hash;
    Ok(receipt)
}

fn collect_conversation_observations(
    store: &impl Store,
    state: &SessionState,
) -> ConversationObservations {
    let mut assistant_fragments = Vec::new();
    let mut tools = BTreeSet::new();
    let mut tool_outputs = Vec::new();
    let mut tool_arguments = Vec::new();

    for blob_ref in &state.conversation_message_refs {
        let Ok(value) = load_json_blob(store, blob_ref) else {
            continue;
        };
        walk_message_value(
            &value,
            false,
            &mut assistant_fragments,
            &mut tools,
            &mut tool_outputs,
            &mut tool_arguments,
        );
    }

    assistant_fragments.sort();
    assistant_fragments.dedup();
    tool_outputs.sort();
    tool_outputs.dedup();
    tool_arguments.sort();
    tool_arguments.dedup();
    ConversationObservations {
        assistant_text: assistant_fragments.join("\n"),
        tool_names: tools,
        tool_outputs,
        tool_arguments,
    }
}

fn walk_message_value(
    value: &Value,
    in_assistant: bool,
    assistant_fragments: &mut Vec<String>,
    tools: &mut BTreeSet<String>,
    tool_outputs: &mut Vec<String>,
    tool_arguments: &mut Vec<String>,
) {
    match value {
        Value::Object(map) => {
            let role_assistant = map
                .get("role")
                .and_then(Value::as_str)
                .is_some_and(|role| role.eq_ignore_ascii_case("assistant"));

            if map
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind == "tool_call")
                && let Some(name) = map.get("name").and_then(Value::as_str)
            {
                tools.insert(name.to_string());
            }

            if let Some(calls) = map.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    let Some(call_obj) = call.as_object() else {
                        continue;
                    };
                    let Some(name) = call_obj.get("name").and_then(Value::as_str) else {
                        continue;
                    };
                    tools.insert(name.to_string());
                    if let Some(arguments) = call_obj.get("arguments") {
                        tool_arguments.push(format!("{} {}", name, value_compact_text(arguments)));
                    }
                }
            }

            if map
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind == "function_call_output")
                && let Some(output) = map.get("output")
            {
                tool_outputs.push(value_compact_text(output));
            }

            let assistant_scope = in_assistant || role_assistant;
            if assistant_scope {
                if let Some(text) = map.get("content").and_then(Value::as_str) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        assistant_fragments.push(trimmed.to_string());
                    }
                }
                if let Some(text) = map.get("text").and_then(Value::as_str) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        assistant_fragments.push(trimmed.to_string());
                    }
                }
                if let Some(text) = map.get("value").and_then(Value::as_str) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        assistant_fragments.push(trimmed.to_string());
                    }
                }
            }

            for child in map.values() {
                walk_message_value(
                    child,
                    assistant_scope,
                    assistant_fragments,
                    tools,
                    tool_outputs,
                    tool_arguments,
                );
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_message_value(
                    item,
                    in_assistant,
                    assistant_fragments,
                    tools,
                    tool_outputs,
                    tool_arguments,
                );
            }
        }
        Value::String(_text) => {}
        _ => {}
    }
}

fn value_compact_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn load_json_blob(store: &impl Store, blob_ref: &str) -> Result<Value> {
    let hash =
        Hash::from_hex_str(blob_ref).with_context(|| format!("invalid hash ref '{blob_ref}'"))?;
    let bytes = store
        .get_blob(hash)
        .with_context(|| format!("load blob {blob_ref}"))?;
    serde_json::from_slice(&bytes).with_context(|| format!("decode json blob {blob_ref}"))
}

fn load_text_blob(store: &impl Store, blob_ref: &str) -> Result<String> {
    let hash =
        Hash::from_hex_str(blob_ref).with_context(|| format!("invalid hash ref '{blob_ref}'"))?;
    let bytes = store
        .get_blob(hash)
        .with_context(|| format!("load blob {blob_ref}"))?;
    String::from_utf8(bytes).with_context(|| format!("decode utf8 blob {blob_ref}"))
}

fn store_json_blob(store: &impl Store, value: &Value) -> Result<HashRef> {
    let bytes = serde_json::to_vec(value).context("encode json blob")?;
    let hash = store.put_blob(&bytes).context("store json blob")?;
    HashRef::new(hash.to_hex()).context("json blob hash_ref")
}

fn make_adapter<S: Store + 'static>(
    store: std::sync::Arc<S>,
    provider: &ProviderRuntime,
) -> LlmAdapter<S> {
    let mut providers = HashMap::new();
    providers.insert(
        provider.provider_id.clone(),
        ProviderConfig {
            base_url: provider.base_url.clone(),
            timeout: std::time::Duration::from_secs(120),
            api_kind: provider.api_kind,
        },
    );
    let config = LlmAdapterConfig {
        providers,
        default_provider: provider.provider_id.clone(),
    };
    LlmAdapter::new(store, config)
}

fn resolve_provider(
    provider: ProviderChoice,
    model_override: Option<String>,
) -> Result<ProviderRuntime> {
    match provider {
        ProviderChoice::Openai => {
            let api_key = env_or_dotenv_var(OPENAI_KEY_ENV)
                .ok_or_else(|| anyhow!("missing {} (env or .env)", OPENAI_KEY_ENV))?;
            let model = model_override.unwrap_or_else(|| "gpt-5.3-codex".into());
            let base_url = env_or_dotenv_var(OPENAI_BASE_URL_ENV)
                .unwrap_or_else(|| "https://api.openai.com/v1".into());
            Ok(ProviderRuntime {
                provider_id: "openai-responses".into(),
                api_kind: LlmApiKind::Responses,
                api_key,
                model,
                base_url,
            })
        }
        ProviderChoice::Anthropic => {
            let api_key = env_or_dotenv_var(ANTHROPIC_KEY_ENV)
                .ok_or_else(|| anyhow!("missing {} (env or .env)", ANTHROPIC_KEY_ENV))?;
            let model = model_override.unwrap_or_else(|| {
                env_or_dotenv_var(ANTHROPIC_MODEL_ENV).unwrap_or_else(|| "claude-sonnet-4-5".into())
            });
            let base_url = env_or_dotenv_var(ANTHROPIC_BASE_URL_ENV)
                .unwrap_or_else(|| "https://api.anthropic.com/v1".into());
            Ok(ProviderRuntime {
                provider_id: "anthropic".into(),
                api_kind: LlmApiKind::AnthropicMessages,
                api_key,
                model,
                base_url,
            })
        }
    }
}

fn env_or_dotenv_var(key: &str) -> Option<String> {
    if let Ok(value) = std::env::var(key) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    for path in dotenv_candidates() {
        let Ok(contents) = fs::read_to_string(path) else {
            continue;
        };
        if let Some(value) = parse_dotenv_value(&contents, key) {
            return Some(value);
        }
    }
    None
}

fn dotenv_candidates() -> Vec<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    vec![
        workspace_root().join(".env"),
        manifest_dir.join(".env"),
        PathBuf::from(".env"),
    ]
}

fn parse_dotenv_value(contents: &str, key: &str) -> Option<String> {
    for raw_line in contents.lines() {
        let mut line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(stripped) = line.strip_prefix("export ") {
            line = stripped.trim();
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() != key {
            continue;
        }
        let value = value.trim();
        let unquoted = if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
            || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
        {
            &value[1..value.len() - 1]
        } else {
            value
        };
        if !unquoted.is_empty() {
            return Some(unquoted.to_string());
        }
    }
    None
}

fn preview(text: &str) -> String {
    let trimmed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.len() <= 800 {
        trimmed
    } else {
        format!("{}...", &trimmed[..800])
    }
}

fn sanitize_case_id(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "case".to_string()
    } else {
        trimmed.to_string()
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("parent of crate dir")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::{value_compact_text, walk_message_value};
    use serde_json::json;
    use std::collections::BTreeSet;

    #[test]
    fn walk_message_value_collects_tool_calls_and_outputs_from_follow_up_shape() {
        let value = json!({
            "role": "assistant",
            "tool_calls": [
                {
                    "id": "call_1",
                    "name": "read_file",
                    "arguments": { "path": "jobs/task-418.json" }
                }
            ],
            "content": "working"
        });
        let output = json!({
            "type": "function_call_output",
            "call_id": "call_1",
            "output": {"tool":"read_file","ok":true},
            "is_error": false
        });

        let mut assistant = Vec::new();
        let mut tools = BTreeSet::new();
        let mut tool_outputs = Vec::new();
        let mut tool_arguments = Vec::new();

        walk_message_value(
            &value,
            false,
            &mut assistant,
            &mut tools,
            &mut tool_outputs,
            &mut tool_arguments,
        );
        walk_message_value(
            &output,
            false,
            &mut assistant,
            &mut tools,
            &mut tool_outputs,
            &mut tool_arguments,
        );

        assert!(tools.contains("read_file"));
        assert!(assistant.iter().any(|item| item == "working"));
        assert!(
            tool_arguments
                .iter()
                .any(|item| item.contains("read_file") && item.contains("jobs/task-418.json"))
        );
        assert!(
            tool_outputs
                .iter()
                .any(|item| item.contains("\"tool\":\"read_file\""))
        );
    }

    #[test]
    fn value_compact_text_keeps_strings_plain() {
        assert_eq!(value_compact_text(&json!("alpha")), "alpha");
        assert_eq!(value_compact_text(&json!({"a":1})), "{\"a\":1}");
    }
}
