# World Host

Short version: I’d go **hybrid** and make that explicit in the runtime:

* A **long‑lived “world runner” process** as the *normal* way to run a world (timers, adapters, REPL, etc).
* A **single-shot “step” mode** driven by the same kernel API, for tests, CI, and quick one-off CLI commands.
* A **thin, pleasant CLI** that can either:

  * talk to a running world over a local channel, or
  * spin up a world, run a step, and shut it down.

The trick is to design the kernel API so both modes are just different wrappers over the same thing.

---

## 1. Grounding: what a “world” actually needs to do

From your docs, a world is:

* A **single-threaded deterministic stepper** over an append-only journal + snapshots. 
* All state change happens via **events → reducers**, with plans orchestrating effects and receipts closing the loop.
* There’s already the idea that tools/CLI provide lifecycle ops like `world init/info`, `propose/shadow/approve/apply`, `run/tail`, etc. 

The only missing piece is: *how is that stepper actually hosted*? Right now, as you said, it’s basically embedded test harnesses like `ExampleReducerHarness` and bespoke runners (e.g. your `llm_summarizer` harness) that:

* Construct a kernel/world instance in-process
* Manually inject events/effect receipts
* Drive the stepper to quiescence
* Inspect state

That’s perfect for tests… but not enough for a “real” world that needs:

* **Timer delivery** (`timer.set` → `TimerFired@1` events) 
* Continuous integration adapters (HTTP inbox, LLM adapter, etc.)
* Long-lived plans that `await_event` or `await_receipt` over minutes/hours

So you’re totally right: you need at least *one* proper “world as a process” story.

---

## 2. The core abstraction: a `WorldHost` you can run in different modes

Before deciding CLI vs daemon, it helps to explicitly define a host API the kernel exposes. Something along these lines (conceptually):

```rust
pub struct WorldHost<S: Store> {
    kernel: Kernel<S>,
    store: Arc<S>,
    adapters: AdapterRegistry,
    config: HostConfig,  // retained for introspection and adapter config
}

impl<S: Store> WorldHost<S> {
    pub fn open(store: Arc<S>, path: &Path, config: HostConfig) -> Result<Self>;

    /// Access host configuration
    pub fn config(&self) -> &HostConfig;

    /// Apply one event (DomainEvent, Receipt, Governance event, etc.)
    pub fn enqueue_external(&mut self, evt: ExternalEvent) -> Result<()>;

    /// Run the deterministic stepper until no more ready work
    pub fn drain(&mut self) -> Result<DrainOutcome>;

    /// Query reducer state (key is for future keyed reducers/cells)
    pub fn state(&self, reducer: &str, key: Option<&[u8]>) -> Option<&[u8]>;

    /// Run one complete cycle: drain → dispatch effects → apply receipts → drain again
    /// This is the shared entry point for batch/daemon/REPL modes.
    pub async fn run_cycle(&mut self) -> Result<CycleOutcome>;

    /// Create a snapshot
    pub fn snapshot(&mut self) -> Result<()>;
}
```

Notes:
- Kernel already owns manifest load/validation, journal + snapshot I/O, deterministic stepping, effect queueing, receipt application, and state queries; the host should delegate rather than duplicate.
- Kernel has no fuel concept; if you want guardrails, count ticks in the host and stop after N.
- **No separate `pending_effects()`**: The kernel's `drain_effects()` clears the queue (and snapshots capture queued intents). A non-destructive peek would break replay semantics. Instead, `run_cycle()` takes ownership of drained intents internally.
- **Keyed state**: The `key` parameter is forward-compatible with keyed reducers (cells). `DomainEvent` already carries an optional key, and manifests declare `key_field`, but routing ignores it today.
- **Adapter errors → receipts**: `AdapterRegistry::execute_batch` always returns receipts (never `Err`). Adapter failures become `ReceiptStatus::Error` or `ReceiptStatus::Timeout` receipts. This ensures the kernel's invariants hold: every intent gets a receipt, plans/reducers unblock, and replay is deterministic.

That’s roughly what you already have conceptually in the architecture doc; just make it a **first-class runtime struct** that can be:

* embedded in tests,
* embedded in a long-lived daemon process,
* or used for single-shot CLI invocations.

Once that exists, “run-modes” are just:

* *how often* you call `drain()`
* *how* you feed `enqueue_external_event` and `apply_receipt`
* and *who* owns the process lifetime.

---

## 3. Two primary run modes

### Mode A — **Long-lived world runner** (daemon-ish)

Think: `aos world run ./worlds/demo`.

Characteristics:

* Starts a `WorldHost` in a process.

* Spins a simple scheduler loop:

  * read new journal entries / external commands
  * drive the kernel (`drain()`)
  * flush new EffectIntents to adapters
  * wait for receipts (HTTP, LLM, timers, etc.)
  * append receipts, loop

* Owns:

  * Timer adapter (real OS timers that eventually call back with receipts).
  * HTTP/LLM/etc adapters.
  * A local control channel (Unix socket / TCP / stdin REPL) for:

    * sending DomainEvents into the world,
    * doing design-time ops (propose/shadow/apply),
    * read-only queries (possibly via the `StateReader` HTTP surface you sketched). 

**Why you almost certainly want this mode:**

* **Timers become trivial**: `timer.set` schedules a host timer, host later injects a `Receipt` for that intent. No extra concept.
* **Long-lived plans** work naturally (they’re just paused in the log until a receipt or DomainEvent wakes them).
* **Nice REPLs**: terminal-based, HTTP-based, or even an LLM REPL can stream commands into a running world without paying the load/replay cost per keystroke.
* It’s the most obvious story for “a world is running somewhere and doing stuff”.

From a DX standpoint, this is your “happy path”:

```bash
# Create sandbox world from a template manifest
aos world init worlds/demo --template=llm-repl

# Run it interactively
aos world run worlds/demo   # prints logs; Ctrl-C = clean shutdown
```

Then your REPL tool can simply talk to `world run` over a socket or stdin.

---

### Mode B — **Single-shot “batch step”** (CLI-driven)

Think: `aos world exec ./worlds/demo --event demo/Foo@1 '{"x": 1}'`.

This mode:

1. Opens the world directory.
2. Loads the last snapshot + replays the tail (if needed) to get a live `WorldHost`.
3. Injects one or more external events:

   * DomainEvent(s) (e.g. “user clicked X”)
   * Receipts from adapters (e.g. from a cron-driven HTTP poller)
   * Governance events (Proposed / Approved / Applied)
4. Calls `drain()` until quiescent.
5. Writes out any new snapshots, journal segments.
6. Exits.

**Pros:**

* Super simple mental model for dev tools and CI:

  * “Every CLI call is one deterministic batch step.”
* Great for **tests and scripts**:

  * `aos world exec` as a hermetic step in a CI job.
* For small worlds, load+replay overhead is acceptable, and running worlds “serverless style” (on-demand) is actually pretty appealing.

**Cons:**

* **Timers don’t really “fire themselves”**; you’d need some external cron-like thing to:

  * read pending timers (from an external view of the timer adapter),
  * at the right time, call `aos world exec --inject-receipt ...`.
* Long-running flows are clunkier; you’re now “faking a daemon” with cron/SQS/etc.

My view: still *very* worth having, but as a **secondary mode** that shares all its implementation with the long-lived runner.

---

## 4. A hybrid that doesn’t suck: same engine, two harnesses

Rather than choosing one forever, you can make both feel first-class by:

1. **Centering everything on `WorldHost`** (or whatever you call the kernel host API).

2. Writing two thin harnesses:

   * `worldd`: the long-lived runner.
   * `world-step`: the batch CLI wrapper.

3. Teaching the CLI to automatically pick its path:

   * `aos world run PATH` → spawn `worldd`.
   * `aos world step PATH ...` → call `world-step` (single-shot).
   * `aos world exec` / `aos world repl`:

     * If a `worldd` is already running for that path, talk to it.
     * Otherwise, either:

       * auto-start a runner (dev-friendly), or
       * fall back to a `world-step` invocation (CI-friendly).

This is similar to how things like `docker` vs `dockerd` or `git` vs a local SSH agent are split: the UX exposes “a thing you talk to”; behind the scenes it may start a daemon or hit disk directly.

Key point: **don’t fork the kernel logic**. Your existing harness and the future CLI/daemon should all go through the same `WorldHost` abstraction.

---

## 5. Interaction paradigms: sending stuff *into* a world

You mentioned:

> how to send commands/events/pokes to a world from the outside.

You already have the abstractions; you just need to surface them:

* **Design-time control-plane ops** (propose/shadow/approve/apply) are just events in the journal; CLI wrappers can call into the runtime to append them.
* **Runtime DomainEvents**: external events that look like “user did X” or “HTTP inbox message arrived”.
* **Effect receipts**: adapters injecting results and costs.

I’d define a small “external API surface” for the runner (used by CLI, REPL, HTTP):

```text
Command:
  - propose {patch_doc}
  - approve {proposal_id, decision}
  - apply {proposal_id}
  - send-event {schema, value_json[, key]}
  - inject-receipt {intent_hash, receipt_json}
  - query-state {reducer, key?, at_least_height?}
  - query-manifest
```

On a long-lived runner, that’s just JSON or CBOR over a local socket; on the batch CLI it’s direct function calls.

This also dovetails nicely with your **read-only query surfaces** design:

* `query-state` and `query-manifest` can literally be backed by the `StateReader` trait you described (hot/warm/cold paths, `Head` vs `AtLeast(height)`, etc.). 

---

## 6. Timers and adapters: why they push you toward a long-lived runner

`timer.set` in v1 is a reducer micro-effect: reducer emits `timer.set`, adapter turns it into a OS timer, and later returns a `TimerSetReceipt` → kernel converts that into `sys/TimerFired@1` for the reducer.

This is *so* much easier when:

* There is a process that:

  * knows which world issued the timer,
  * keeps a heap of pending deadlines,
  * and, when a deadline hits, injects a receipt into the world.

Trying to keep worlds fully cold and just tick them with a cron job basically forces you to duplicate half the “effect manager + adapters + timers” logic outside the core.

So the pragmatic approach:

* **Phase 1:** assume timers only really work when a world is running in `world run` mode.
* **Phase 2:** if you want serverless-style worlds, make the timer adapter persistent and let it call `aos world exec --inject-receipt` per fire.

But that’s a layering concern, not a kernel concern. The kernel doesn’t care if `ReceiptAppended` came from a daemon or a cron.

---

## 7. How this plays with the LLM terminal REPL

You had:

> One of the first app … is a LLM based REPL that run in the terminal (adding terminal effect adapters), and then interact with a very simple AOS based agent.

Given the run-modes above, here’s how I’d wire that:

1. **Define a tiny “terminal” effect & capability**:

   * `EffectKind`: `terminal.print`, `terminal.readline` (or just a simple `terminal.write` plus the REPL driver lives outside).
   * A `terminal` adapter that talks to stdin/stdout.
   * For governance, that’s just another adapter+defeffect+defcap pair.

2. **Create a simple world manifest**:

   * One reducer `demo/ChatSM@1` that:

     * holds conversation state,
     * emits DomainIntents when it needs LLM work.
   * One plan `demo/chat_llm@1` that:

     * on intent, emits `llm.generate`,
     * awaits receipt,
     * raises a `ChatModelReplied` event back to `ChatSM`.
   * Optional plan that sends friendly formatted text to `terminal.print`.

3. **Run a world runner process**:

   ```bash
   aos world init worlds/chat
   aos world run worlds/chat
   ```

4. **Have your REPL program talk to the runner**:

   * Read a user line.
   * Send a `ChatUserMessage` DomainEvent into the world via your control channel.
   * Optionally block until:

     * the world has drained, and
     * a certain reducer state field (`last_bot_message`) changed.
   * Print it.

For this style of REPL, the long-lived runner is clearly the right mode. However, the *same world* can still be driven in CI by `aos world step` with canned events and receipts.

---

## 8. DX and exploration: what to make nice *first*

If we optimize for “joy to use” in the next 1–2 iterations, I’d prioritise:

### 8.1 A single, obvious “dev command”

Something like:

```bash
aos dev worlds/demo
```

That:

* If `worlds/demo` doesn’t exist:

  * scaffolds a new world from a template:

    * minimal manifest,
    * one reducer+plan pair,
    * default caps/policy.
* Starts a world runner with pretty logging:

  * show each event, each effect, each receipt in a compact timeline.
* Drops you into a **REPL-like prompt** that is just sugar over the world control channel:

  * `event demo/Foo@1 { ... }`
  * `state demo/MyReducer@1`
  * `plan demo/some_plan@1 { ... }` (manual start)
  * maybe `llm` to send a line into a prewired chat agent.

Under the hood, this uses the **same runner** as `aos world run`.

### 8.2 A clean test harness that mirrors CLI semantics

Right now you have test harnesses (`ExampleReducerHarness`, `HttpHarness`, etc.). I’d:

* Rebuild them on top of `WorldHost`.
* Have them *literally* call into the same API your CLI uses:

  * `enqueue_external_event`
  * `drain`
  * `StateReader` queries

So when devs write tests, they’re learning the same mental model the runtime uses. No “special test-only semantics”.

### 8.3 Batch-mode CLI for CI

Provide a small stable surface that CI can rely on, e.g.:

```bash
aos world step ./worlds/demo \
  --event demo/OrderCreated@1 @tests/order1.json \
  --assert-state demo/OrderSM@1 'pc == "Paid"'
```

Where `--assert-state` is implemented in terms of `StateReader` and a tiny embedded expression evaluator (or just `jq` if you serialize state as JSON).

This is where the single-shot mode shines: CI doesn’t want daemons; it wants one-shot steps with deterministic outputs.

---

## 9. Concrete recommendation / roadmap

Putting it all together, here’s a practical path:

### Phase 0: Extract the runtime host API

* Extract what your current harnesses are doing into a `WorldHost` (or `KernelHost`, etc.).
* Make sure it owns:

  * journal I/O,
  * snapshotting,
  * manifest loading/validation,
  * reducer + plan stepping,
  * effect emission + pending queue.

### Phase 1: Implement **two harnesses** over that API

1. `world-step`:

   * Opens world → applies requested external events → drains → writes snapshot → exits.
2. `worldd`:

   * Opens world → event loop:

     * accept control commands over a simple protocol,
     * run `drain()` when there’s work,
     * talk to adapters (including timer).

### Phase 2: Build the CLI on top

* `aos world init`
* `aos world run` → invokes/embeds `worldd`.
* `aos world step` / `aos world exec` → invokes `world-step`.
* `aos dev` → sugar for `world run` with a friendly REPL.

### Phase 3: Wire the LLM REPL

* Add `terminal` + `llm` adapters.
* Ship a tiny “chat world” template.
* Make `aos dev` default to that template if nothing exists.

At that point you’ll have:

* A **real runtime harness** that feels like a normal daemon.
* A **batch mode** that keeps your replay story trivial and is perfect for tests.
* A **nice exploration loop**: `aos dev` to poke a world, introspect events, and watch an agent do stuff.

And crucially: you haven’t made a hard choice between “always-running” vs “CLI-per-step”. You’ve made **run-modes a hosting concern** over a single deterministic kernel, which fits the AgentOS architecture really cleanly.

---

## Open decisions (control channel)

- **Transport:** default to Unix socket vs stdin/stdout; where to place the socket (world dir? tmp? env override?); leave room for TCP if ever remote.
- **Framing:** JSON Lines for dev-friendliness vs CBOR for compactness; likely JSON first, optional CBOR later.
- **Command surface (MVP):** send-event, inject-receipt, query-state, query-manifest; governance verbs (propose/approve/apply) to be confirmed after P3.
- **Auth:** currently assumes local trusted caller; if remote is allowed, decide on auth (mTLS/token) and capability scoping.
- **Versioning:** include a protocol version field so CLI/REPL/tests can negotiate changes.

---

## Design decisions (resolved)

These decisions were made after reviewing the kernel implementation and considering replay/snapshot semantics:

### 1. API naming: `enqueue_external` (not `enqueue`)

The kernel already distinguishes external inputs (`submit_domain_event`, `handle_receipt`) from internal routing (`enqueue_event`). The host API mirrors this: `enqueue_external` makes the boundary explicit and avoids confusion once governance/internal events are exposed.

### 2. Store `HostConfig` on `WorldHost`

`HostConfig` is passed to `open()` and retained as a field. This is needed for:
- Adapter configuration (timeouts, endpoints, model defaults)
- REPL introspection (`config show`)
- Runtime diagnostics

### 3. No separate `pending_effects()` method

The kernel's `drain_effects()` **clears the queue**, and snapshots capture queued intents ([world.rs:448-450, 484-488](crates/aos-kernel/src/world.rs#L448-L488)). A "non-destructive" `pending_effects()` would double-dispatch after receipts and wouldn't rehydrate on replay.

Instead, `run_cycle()` takes ownership of drained intents internally. If batch/daemon/REPL need to inspect intents (for logging, filtering), they can do so on the owned `Vec<EffectIntent>` after calling `kernel.drain_effects()` directly.

### 4. Keyed state queries: include `key` parameter now

`DomainEvent` already carries an optional `key` field ([aos-wasm-abi/src/lib.rs:33-83](crates/aos-wasm-abi/src/lib.rs#L33-L83)), and manifests declare `key_field`, but routing ignores it today ([world.rs:242-249](crates/aos-kernel/src/world.rs#L242-L249)). Including `key: Option<&[u8]>` in the API now avoids churn when keyed routing (cells) is implemented.

### 5. Adapter errors always become receipts

`handle_receipt` is the **only** thing that clears `pending_receipts`/`pending_reducer_receipts` and unblocks plans/reducers ([world.rs:1159-1197](crates/aos-kernel/src/world.rs#L1159-L1197)). If `execute_batch` returned `Err` and we dropped the intent, the world would hang forever — and a snapshot wouldn't requeue it.

Design:
- **Adapter failure** (HTTP 500, rate limit, etc.) → `ReceiptStatus::Error` receipt with error info in payload
- **Adapter timeout** → `ReceiptStatus::Timeout` receipt (synthesized by host)
- **Adapter unreachable** → `ReceiptStatus::Error` receipt (synthesized by host)
- **True host failure** (process crash, OOM) → out of scope; no receipt, world waits until restart

This keeps the kernel's invariants intact across restarts.

### 6. Shared `run_cycle()` method

The drain → dispatch → apply receipts → drain loop is needed by:
- `BatchRunner::step()` (P1)
- `WorldDaemon::tick()` (P2)
- REPL `cmd_step` (P4)

Rather than duplicating this logic, `WorldHost::run_cycle()` is the single implementation. Mode-specific wrappers can add their own concerns (snapshotting, logging, error handling) around it.
