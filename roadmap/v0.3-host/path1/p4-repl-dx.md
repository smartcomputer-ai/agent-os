# P4: REPL & Developer Experience

**Goal:** Provide a pleasant interactive loop (`aos dev`) that talks to a running world (auto-starts daemon if needed) using the same control channel.

## REPL Implementation

### Structure

```
crates/aos-host/src/repl/
├── mod.rs          # ReplSession, main loop
├── commands.rs     # Command handlers
├── display.rs      # Pretty formatting
└── parser.rs       # Command parsing
```

### ReplSession

```rust
// repl/mod.rs
use rustyline::Editor;
use rustyline::error::ReadlineError;

pub struct ReplSession<S: Store + 'static> {
    host: WorldHost<S>,
    timer_heap: Arc<Mutex<TimerHeap>>,
    editor: Editor<()>,
    history_path: PathBuf,
}

impl<S: Store + 'static> ReplSession<S> {
    pub fn new(host: WorldHost<S>, timer_heap: Arc<Mutex<TimerHeap>>) -> Self {
        let mut editor = Editor::<()>::new().expect("create editor");
        let history_path = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("aos")
            .join("repl_history");

        // Load history
        let _ = editor.load_history(&history_path);

        Self { host, timer_heap, editor, history_path }
    }

    pub async fn run(&mut self) -> Result<(), HostError> {
        println!("AgentOS REPL - type 'help' for commands");
        println!();

        loop {
            let prompt = format!("aos> ");
            match self.editor.readline(&prompt) {
                Ok(line) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }

                    self.editor.add_history_entry(line);

                    match self.execute_command(line).await {
                        Ok(true) => continue,  // command succeeded, continue
                        Ok(false) => break,    // quit command
                        Err(e) => println!("Error: {}", e),
                    }
                }
                Err(ReadlineError::Interrupted) => {
                    println!("^C");
                    continue;
                }
                Err(ReadlineError::Eof) => {
                    println!("Bye!");
                    break;
                }
                Err(e) => {
                    println!("Error: {:?}", e);
                    break;
                }
            }
        }

        // Save history
        let _ = self.editor.save_history(&self.history_path);
        Ok(())
    }

    async fn execute_command(&mut self, input: &str) -> Result<bool, HostError> {
        let parts: Vec<&str> = input.splitn(2, ' ').collect();
        let cmd = parts[0];
        let args = parts.get(1).unwrap_or(&"");

        match cmd {
            "help" | "?" => self.cmd_help(),
            "quit" | "exit" | "q" => return Ok(false),
            "event" | "e" => self.cmd_event(args).await?,
            "state" | "s" => self.cmd_state(args)?,
            "effects" => self.cmd_effects()?,
            "timers" => self.cmd_timers()?,
            "step" => self.cmd_step().await?,
            "run" => self.cmd_run().await?,
            "snapshot" => self.cmd_snapshot()?,
            "manifest" => self.cmd_manifest()?,
            _ => println!("Unknown command: {}. Type 'help' for commands.", cmd),
        }

        Ok(true)
    }
}
```

### Commands

```rust
// repl/commands.rs
impl<S: Store + 'static> ReplSession<S> {
    fn cmd_help(&self) {
        println!("Commands:");
        println!("  event <schema> <json>  - Send a domain event");
        println!("  state <reducer>        - Query reducer state");
        println!("  effects                - Show pending effects");
        println!("  timers                 - Show pending timers");
        println!("  step                   - Drain until idle");
        println!("  run                    - Run continuously (Ctrl-C to stop)");
        println!("  snapshot               - Create a snapshot");
        println!("  manifest               - Show manifest info");
        println!("  help                   - Show this help");
        println!("  quit                   - Exit REPL");
    }

    async fn cmd_event(&mut self, args: &str) -> Result<(), HostError> {
        // Parse: schema json
        let parts: Vec<&str> = args.splitn(2, ' ').collect();
        if parts.len() < 2 {
            println!("Usage: event <schema> <json>");
            return Ok(());
        }

        let schema = parts[0];
        let json_str = parts[1];

        // Parse JSON to CBOR
        let value: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| HostError::InvalidInput(e.to_string()))?;
        let cbor = serde_cbor::to_vec(&value)?;

        self.host.enqueue(ExternalEvent::DomainEvent {
            schema: schema.to_string(),
            value: cbor,
        })?;

        println!("Event queued: {}", schema);

        // Auto-step
        self.cmd_step().await?;
        Ok(())
    }

    fn cmd_state(&self, args: &str) -> Result<(), HostError> {
        let reducer = args.trim();
        if reducer.is_empty() {
            // List all reducers
            println!("Reducers:");
            // TODO: iterate over manifest reducers
            return Ok(());
        }

        match self.host.state(reducer) {
            Some(bytes) => {
                // Try to decode as JSON for pretty printing
                match serde_cbor::from_slice::<serde_json::Value>(bytes) {
                    Ok(value) => {
                        println!("{}", serde_json::to_string_pretty(&value)?);
                    }
                    Err(_) => {
                        println!("(raw bytes, {} bytes)", bytes.len());
                    }
                }
            }
            None => println!("Reducer not found: {}", reducer),
        }
        Ok(())
    }

    fn cmd_effects(&self) -> Result<(), HostError> {
        let effects = self.host.pending_effects();
        if effects.is_empty() {
            println!("No pending effects");
        } else {
            println!("Pending effects:");
            for intent in effects {
                println!("  - {} (hash: {})",
                    intent.kind,
                    hex::encode(&intent.intent_hash[..8])
                );
            }
        }
        Ok(())
    }

    fn cmd_timers(&self) -> Result<(), HostError> {
        let heap = self.timer_heap.lock().unwrap();
        let timers = heap.pending();
        if timers.is_empty() {
            println!("No pending timers");
        } else {
            println!("Pending timers:");
            for entry in timers {
                let until = entry.deadline.saturating_duration_since(Instant::now());
                println!("  - fires in {:?} (id: {:?})",
                    until,
                    entry.timer_id
                );
            }
        }
        Ok(())
    }

    async fn cmd_step(&mut self) -> Result<(), HostError> {
        // Fire due timers
        {
            let mut heap = self.timer_heap.lock().unwrap();
            let fired = self.host.fire_due_timers(&mut heap)?;
            if fired > 0 {
                println!("Fired {} timer(s)", fired);
            }
        }

        // Drain and execute
        let outcome = self.host.drain()?;
        println!("Drained: {} ticks", outcome.ticks);

        let intents = self.host.pending_effects();
        let receipts = self.host.dispatch_effects(intents).await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        if !receipts.is_empty() {
            println!("Executed {} effect(s)", receipts.len());
            for receipt in receipts {
                self.host.enqueue(ExternalEvent::Receipt(receipt))?;
            }
            // Drain again after receipts
            let outcome2 = self.host.drain()?;
            if outcome2.ticks > 0 {
                println!("Drained: {} more ticks", outcome2.ticks);
            }
        }

        Ok(())
    }

    async fn cmd_run(&mut self) -> Result<(), HostError> {
        println!("Running continuously (Ctrl-C to stop)...");

        let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(1);

        // Spawn Ctrl-C handler
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            tx.send(()).await.ok();
        });

        loop {
            tokio::select! {
                _ = rx.recv() => {
                    println!("\nStopped");
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    self.cmd_step().await?;
                }
            }
        }

        Ok(())
    }

    fn cmd_snapshot(&mut self) -> Result<(), HostError> {
        self.host.snapshot()?;
        println!("Snapshot created");
        Ok(())
    }

    fn cmd_manifest(&self) -> Result<(), HostError> {
        // Show manifest summary
        let kernel = self.host.kernel();
        println!("Manifest:");
        println!("  Reducers: ...");  // TODO: expose from kernel
        println!("  Plans: ...");
        Ok(())
    }
}
```

### Pretty Display

```rust
// repl/display.rs
use colored::Colorize;

pub fn format_event(schema: &str, value: &[u8]) -> String {
    format!("{} {}", "EVENT".green().bold(), schema.cyan())
}

pub fn format_effect(kind: &str, hash: &[u8]) -> String {
    format!("{} {} ({})",
        "EFFECT".yellow().bold(),
        kind.cyan(),
        hex::encode(&hash[..4])
    )
}

pub fn format_receipt(adapter: &str, status: &ReceiptStatus) -> String {
    let status_str = match status {
        ReceiptStatus::Ok => "OK".green(),
        ReceiptStatus::Error => "ERROR".red(),
        ReceiptStatus::Timeout => "TIMEOUT".yellow(),
    };
    format!("{} {} [{}]", "RECEIPT".blue().bold(), adapter, status_str)
}
```

## CLI `aos dev` Command

```rust
// cli/commands.rs additions
#[derive(Subcommand)]
pub enum Commands {
    // ... existing commands ...

    /// Interactive development mode
    Dev {
        #[arg()]
        path: PathBuf,

        /// Template to use if creating new world
        #[arg(long, default_value = "minimal")]
        template: String,
    },
}

async fn run_dev(path: &Path, template: &str) -> Result<()> {
    // Scaffold if doesn't exist
    if !path.exists() {
        println!("Creating new world from template '{}'...", template);
        scaffold_world(path, template)?;
    }

    // Open host
    let store = Arc::new(FsStore::open(path)?);
    let config = HostConfig::from_env()?;

    let timer_adapter = TimerAdapter::new();
    let timer_heap = timer_adapter.heap();

    let mut host = WorldHost::open(store, &path.join("manifest.air.json"), config)?;
    host.adapters.register(Box::new(timer_adapter));
    host.adapters.register(Box::new(HttpAdapter::new(HttpAdapterConfig::default())));
    if let Ok(llm) = LlmAdapter::from_env() {
        host.adapters.register(Box::new(llm));
    }

    // Start REPL
    let mut session = ReplSession::new(host, timer_heap);
    session.run().await?;

    Ok(())
}

fn scaffold_world(path: &Path, template: &str) -> Result<()> {
    std::fs::create_dir_all(path)?;
    std::fs::create_dir_all(path.join(".aos"))?;

    // Write minimal manifest based on template
    let manifest = match template {
        "minimal" => minimal_manifest(),
        "counter" => counter_manifest(),
        "chat" => chat_manifest(),
        _ => return Err(anyhow!("Unknown template: {}", template)),
    };

    std::fs::write(
        path.join("manifest.air.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;

    Ok(())
}
```

## Dependencies

```toml
# Additional dependencies
rustyline = "14"
colored = "2"
hex = "0.4"
dirs = "5"
```

## Tasks

1) Implement control-channel client reused by CLI/REPL (Unix socket or stdin JSON).
2) Map REPL commands to control ops; add pretty printers for events/effects/receipts/state.
3) Add scaffolding helper for templates (minimal, counter, chat).
4) Wire `aos dev` to auto-start daemon if absent, then launch REPL; Ctrl-C exits REPL and stops auto-started daemon.
5) Persist history under platform data dir.

## Success Criteria

- `aos dev examples/00-counter` lets you `event demo/Increment@1 {}` and see state update.
- Works whether daemon already running or auto-started; exiting REPL shuts down auto-started daemon cleanly.
- History persists across sessions; output is readable/colored.
