## Background
Our vision is to make software development behave more like a compiler pipeline: humans provide intent, constraints, and definitions of “good,” and an automated agent loop turns that into running software with minimal manual coding or line-by-line review. The core thesis is that in an agentic world, correctness can either compound or degrade over iterative model edits, so the system must be designed to systematically drive compounding correctness rather than rely on ad hoc human intervention.

The operating loop is `Seed -> Validation Harness -> Feedback`, with tokens and richer representations (traces, replays, transcripts, simulations) as the fuel that keeps improving outcomes. Validation is intentionally end-to-end, scenario-driven, and close to real usage, including integrations, failure modes, and economics; critically, it uses holdout scenarios that are harder to “game” than in-repo tests. Feedback from failures, near misses, and cost/performance signals is fed back into the next loop until scenarios pass and remain stable over time.

At a system level, our factory treats specs as the durable source of truth, scenarios as the main anti-drift contract, and satisfaction (probabilistic user outcome quality) as a primary metric alongside deterministic checks. It emphasizes Digital Twin validation for safe, high-volume, deterministic testing of external dependencies, unattended “hands-off by default” execution, and a full audit trail of changes, evidence, and judgments. In short, the vision is a spec-first, scenario-governed software factory that can run continuously and safely at scale.

## Goal
What is the path to use AOS as the core runtime for a super-scalable software factory?

The goal is run workloads on a lot of servers in parallel. And have AOS worlds coordinate work.

I'll split the work required into two levels, hosting infra and world capabilities.

## World Features

### Agent
Implement a capable tool use agent that works really solidly with different LLM providers.
Use the forge/attractor agent as a basis and integrate it into aos (it's a rust lib I wrote that works really well with LLMs and can act as a foundation for agents).

Most agent runs will be "headless", so keep that in mind when designing.

The is the agent "SDK" built on top of AOS primitives (schemas, reducers, effects, workspaces, etc). Other agents will be built with it.
- unified LLM API
- streaming
- solid tool use
- parallel tool use

DoD: 
- agent framework exists to build the other agents on top of (which are mostly different tools and prompts, right?)
- test suite that shows that basic agent functionalities all work.

## Coding Agent
An agent that can write and edit code. 

It should be able to work similar to a SOTA coding agent, driving itself forward until task completion.

Mainly designed to touch the external world via tools that the llm has been trained on. So is designed to touch the environment via effect adapters.

**Tools**
1.   read_file  
    Why it is included: Agents need reliable, low-friction code inspection before making changes.  
    What it does: Reads file contents (optionally with offset/limit) and returns text for analysis.
2. write_file  
    Why it is included: Agents need a direct way to create new files or fully replace file contents.  
    What it does: Writes complete content to a path, creating or overwriting the file.
3. edit_file  
    Why it is included: Some model families perform best with exact search/replace style edits.  
    What it does: Replaces old_string with new_string in a file, with ambiguity handling and optional multi-replace.
4. apply_patch  
    Why it is included: OpenAI-aligned coding models are strongest with structured patch-based edits.  
    What it does: Applies patch operations (add/update/delete/move) in one atomic patch workflow.
5. shell  
    Why it is included: Agents must run builds, tests, linters, git commands, and project tooling.  
    What it does: Executes shell commands with timeout control and returns stdout/stderr/exit code.
6. grep  
    Why it is included: Agents need fast semantic/codebase search to locate symbols, patterns, and usages.  
    What it does: Searches file contents by pattern and returns matches.
7. glob  
    Why it is included: Agents need fast file discovery before reading/editing.  
    What it does: Finds files by glob pattern (for example [*.rs](https://file+.vscode-resource.vscode-cdn.net/Users/lukas/.vscode/extensions/openai.chatgpt-0.4.71-darwin-arm64/webview/# "**/*.rs")).
8. spawn_agent  
    Why it is included: Complex tasks benefit from delegation and parallel specialization.  
    What it does: Starts a subagent with a scoped task (optionally model/working-dir bounded).
9. send_input  
    Why it is included: Parent agents need to steer running subagents without restarting them.  
    What it does: Sends a follow-up message/instruction to a live subagent.
10. wait  
    Why it is included: Parent orchestration needs synchronization points.  
    What it does: Waits for a subagent to finish and returns its result/status.
11. close_agent  
    Why it is included: Orchestration needs explicit lifecycle control and cleanup.  
    What it does: Terminates a subagent session.

Provider default note: OpenAI profiles use apply_patch; Anthropic/Gemini profiles use edit_file; all profiles include the shared core and subagent tools.

DoD:
- Can run the agent in a cloned repository and ask it to make changes and it can do it.

### Demiurge Agent
An agent that can modify the world itself, answer questions about the world, debug the world, etc. But also spawn other worlds, etc once universes land. It's the main AOS native agent.

DoD:
- can ask the agent about anything inside the world and it answers correctly
- can modify the world manifest correctly
- can self-write and compile modules and integrate them in the world (compiler effect needed)
- can orchestrate the universe (if caps are granted)

### Other Agents/Worlds
Touches real world artifacts, such as a website, and interacts with it. This is needed to test and gather data for the factory.

World types needed to implement the factory (might or might not be correct, TBD):
- planner
- worker
- judge
- policy/governor
- memory

### Postponed
Let's focus primarily on getting the system to actually work with AI agents! Things like much better cap gating, performance improvements, etc should all be on hold to get to the goal of having a working factory. Of course, anything low-level or pedantic that is in the hot path to the factory should be immediately done.

Open questions: 
- what plan improvements are need to make this work
- do reducers need to be able to emit parallel events/intents instead of just one, or can that be handled by plans? E..g. for parallel tool calls?

## Infrastructure

Stuff needed to build scalable factories.

## Universes
Worlds live in universes and they talk to each other using messages. (see previous universe work)

Things still needed to be designed:
- world ids: how to identify worlds within a universe
- universe naming/ids: what constitutes a universe
- what messages are being sent between worlds?

### Hosting
A shared CAS between worlds within a Universe. Also shared infrastructure for log journals and snapshots. The goal is to allow easy spin up of worlds inside container workloads (and other workloads), and moving them between workloads.

Orchestrator for deciding where a world should run. Should mostly follow a worker model. Because a worker (running as a single process) could run multiple worlds so worlds are more scalable. Worker ensures timers are fired and events are delivered. We want to lean into the  wasm isolation we already have in AOS. The only issue is if a world has effects that can touch, say a file system. But the workker could isolate that too, right? What else?

Orchestrators should be able to also orchestrate multiple universes, we don't want one, say, k8s cluster per universe, that would be wasteful. We'll have hundreds of universes and thousands of worlds to start with! We should plan for this from day one. The factory vision demands it. 

One thing: i would love it if I could also run a AOS workload on my laptop so that agents can interact with my own stuff, not just with k8s/container isolated stuff. This is a stretch goal.

Open questions:
- what persistence technology to use? I'm thinking foundation db...
- what queue system to use? Ideally same as persistence, but can be a different technology.
- how to design the whole db, what protocols to use?
- what transport to use to communicate between worlds?

DoD:
- shared CAS
- shared journal infra
- shared snapshot infra
- orchestrator for universes and worlds

### Control Plane
How to create visibility into all this? We need some sort of visibility of what is scheduled, what is running, how much space is being used, messages being sent, but also what is going on in individual worlds/agents so we can debug and intervene.