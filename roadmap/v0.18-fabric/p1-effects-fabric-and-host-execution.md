# P1: Effects, Fabric, and Host Execution

**Priority**: P1  
**Effort**: Very High  
**Risk if deferred**: High (the system will treat host execution as an afterthought instead of a first-class dark-factory concern)  
**Status**: Partially implemented (inline effect execution and receipt normalization exist on the log-first seam; dedicated effect/fabric lanes and hosted fabric control-plane work remain open)

Carried forward from `roadmap/v0.17-kafka/` now that the dedicated fabric/effect lane,
session/artifact, and secret-provider follow-on work is explicitly deferred to `v0.18-fabric`.

## Goal

Define how the new log-first AOS runtime handles:

- normal effects
- specialized effect workers
- host/container/vm/sandbox execution
- long-running sessions
- logs and artifacts
- secrets

## Completed In Code

Implemented on the experimental branch:

1. The shared log-first runtime already supports inline effect execution through the existing
   adapter registry on `WorldHost`.
2. External effect receipts already have a canonical admission shape via
   `SubmissionPayload::EffectReceipt` and normalize back into authoritative journal history.
3. The local log-first runtime already exposes receipt ingress and records resulting canonical
   `EffectReceipt` journal entries.
4. The hosted log-first worker path can already admit receipt-shaped submissions and normalize them
   into authoritative `WorldLogFrame`s on the partition owner.
5. Existing host/session/exec adapters still provide the current inline execution substrate used by
   local and embedded flows.

Still not completed:

1. Dedicated `aos-effect` and `aos-fabric` lanes with external workers.
2. Hosted receipt/control ingress that is intentionally shaped around the new fabric/effect model
   rather than ad hoc local submission.
3. First-class hosted session, artifact, and log refs/control surfaces on the log-first runtime.
4. Secret-provider cutover for the new hosted architecture.

## Core Design Stance

Host execution is important enough to be treated as a first-class fabric subsystem, not just an
anonymous generic effect adapter.

Worlds decide:

- what should happen
- what execution class they need
- what semantic result they are waiting for

Fabric decides:

- where it runs
- how it is provisioned
- how session state is managed
- where logs and artifacts are stored

## Minimal Topic Strategy

The correctness minimum remains:

- `aos-ingress` for non-authoritative submissions
- `aos-journal` for authoritative world history
- `aos-route` for compacted route metadata

But the design should allow these optional additional lanes:

- `aos-effect`
- `aos-fabric`

Important rule:

- even when those lanes exist, `aos-journal` remains the authoritative world-state log
- external workers submit outcomes through `aos-ingress`; only partition owners commit them into
  `aos-journal`
- `aos-route` remains small current-state metadata, not a second world-history stream
- external protocol messages, worker bookkeeping, and fabric transport envelopes should be treated
  as submission-plane or projection-plane shapes unless the final runtime explicitly promotes a
  semantic subset into canonical world-history records

## Effect Model

### Minimal deployment

World workers may execute effects inline and write resulting receipts directly into the
authoritative `WorldLogFrame` they emit to `aos-journal`.

This path is best treated as:

- local mode behavior
- a simple bring-up path
- a bounded micro-effect path for short, predictable work

It is not the intended production norm for long-running or operationally unpredictable work.

### Production deployment

World workers may instead:

1. write `EffectRequested` to `aos-journal`
2. also publish an effect-dispatch record to `aos-effect`
3. specialized effect workers consume `aos-effect`
4. effect workers execute the side effect
5. effect workers submit the receipt to `aos-ingress`
6. the partition owner admits it and writes authoritative `EffectReceipt` to `aos-journal`

This means:

- the effect lane is not part of the correctness core for world state
- but it is the preferred production path once effect latency or execution variance is
  operationally significant
- partition owners should stay focused on deterministic log progress and partition liveness, not
  absorb arbitrary external execution time
- the authoritative journal should record the minimal semantic intent/receipt history needed for
  replay rather than mirroring effect-lane protocol chatter one-for-one

## Fabric Model

Host execution should be expressed through first-class intent/receipt semantics.

Important production stance:

- host/container/vm/sandbox execution should normally run off the partition owner
- long-running sessions and heavy execution belong on `aos-fabric` style workers
- fabric workers should submit outcomes through `aos-ingress`
- partition owners should observe authoritative fabric receipts/events on `aos-journal`
- the journal vocabulary for fabric should remain intentionally smaller than the transport
  vocabulary used between workers, sessions, and adapters

### Required concepts

- `execution_class`
- `host_session`
- `operation_id`
- `artifact_ref`
- `log_ref`

### Example host intents

- `SessionOpen`
- `WorkspaceMaterialize`
- `ExecRun`
- `ArtifactCollect`
- `ServiceDeploy`
- `ProbeWatch`
- `SessionClose`

### Example host receipts/events

- `Accepted`
- `Started`
- `Progress`
- `Completed`
- `Failed`
- `LogReady`
- `ArtifactReady`
- `DeploymentStatus`

These examples describe the semantic space the runtime may need to express.

They are not a requirement that every transport event, worker callback, or session-level protocol
message be copied into the journal verbatim.

## Session Stance

For dark-factory software workflows, warm execution environments matter.

Therefore:

- one-shot command execution is insufficient as the only model
- host sessions should be first-class external objects
- host sessions should not rely on long-lived execution remaining attached to the partition owner

Sessions let agents reuse:

- cloned repos
- dependency caches
- build artifacts
- deployment handles
- monitoring sessions

The world stores references and semantic progress. The fabric owns the actual external session.

## Artifact And Log Handling

Kafka should not be used as a raw log sink for noisy host output.

Instead:

- Kafka carries semantic lifecycle events and refs
- S3 stores the bulk bytes

Examples:

- stdout/stderr archives
- test reports
- screenshots
- tarballs
- deployment bundles

## Secrets

The new direction should not include an AOS-owned secret provider.

Required stance:

- secret values come from external secret systems
- world state stores only refs, names, and version metadata
- workers/fabric resolve real values at execution time

Possible providers:

- Kubernetes secrets
- cloud secret managers
- vault products
- workload-identity-attached providers

Plaintext secret values must not be carried in Kafka records.

## Idempotency And Failure Semantics

Host execution and many effects are not truly exactly-once.

Therefore:

- every external operation needs a stable `operation_id` or `intent_hash`
- adapters/fabric should use that as the idempotency key when possible
- retries are driven by records and receipts, not hidden mutable queue state

## Out of Scope

1. A first-party secret vault product.
2. Exactly-once guarantees for arbitrary external side effects.
3. Full fabric scheduling optimization policy.
4. Every future execution substrate plugin.

## DoD

1. The roadmap defines specialized effect lanes as optional, not authoritative.
2. The roadmap states that inline execution is primarily for local mode, bring-up, or bounded
   micro-effects, not the general production norm.
3. Host execution is recognized as a first-class fabric subsystem.
4. Sessions, artifacts, and logs are explicitly part of the design.
5. The world log remains the only place where world state advances.
6. Secret handling is defined in terms of external secret providers and refs.
