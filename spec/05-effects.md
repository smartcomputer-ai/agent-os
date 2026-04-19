# Effects and Async Execution

Effects are the boundary between deterministic world execution and everything that can observe or
change the outside world.

The kernel must be replay-identical: given the same manifest, checkpoint/snapshot, journal frames,
and receipts, it must reconstruct the same state. Network calls, timers, LLM providers, host file
tools, blob services, and secret stores do not have that property. They may be slow, fail, retry,
stream partial progress, or return different data over time.

AgentOS handles this by making external work explicit. Workflow modules request work by emitting
typed effect intents. The owner records that work durably before anything outside the world may
start. Async executors perform the work outside the deterministic kernel and return stream frames
or terminal receipts. Those continuations re-enter as ordinary world input and are admitted only by
the owner.

This document defines that effect system.

## 1) Scope

This spec covers:

- effect declarations and runtime classes,
- effect intent canonicalization and authorization,
- durable open work,
- the post-flush publication fence,
- async effect runtime and adapter responsibilities,
- stream frame and receipt admission,
- intent identity, idempotency, ordering, recovery, and quiescence.

It does not define:

- the full AIR effect catalog; see [spec/03-air.md](03-air.md),
- workflow state-machine patterns; see [spec/04-workflows.md](04-workflows.md),
- concrete queue, thread, Tokio task, or deployment topology,
- provider-specific retry, timeout, billing, or API contracts,
- future fabric/session/artifact products.

## 2) Why Effects Exist

The kernel cannot call the network, sleep on a timer, read the host filesystem, invoke an LLM, or
write a secret directly without breaking replay. Those operations depend on wall-clock time,
provider state, process state, credentials, and failure timing.

The effect system gives workflow code a deterministic way to ask for nondeterministic work:

1. The workflow emits an effect intent as data.
2. The kernel validates and records that intent as open work.
3. The node durably appends the resulting world frame.
4. Only after that durable append may an executor start the external operation.
5. The executor returns observed facts as schema-bound stream frames and signed receipts.
6. The owner admits those continuations and advances the world deterministically.

The important design choice is that external work is never hidden behind a normal function call.
Workflow code does not "call HTTP"; it emits a request for `http.request`. The receipt is later
delivered as input. This keeps workflow logic deterministic while still allowing the system to
orchestrate real external systems.

## 3) Vocabulary

**Effect kind**: A namespaced string such as `http.request`, `timer.set`, `llm.generate`,
`blob.put`, `workspace.read_bytes`, or `introspect.workflow_state`.

**Effect catalog entry**: A `defeffect` node that binds an effect kind to parameter and receipt
schemas, a capability type, and an origin scope.

**Effect intent**: A canonical request to perform one effect. It includes the effect kind,
canonical params, capability grant name, origin metadata, idempotency input, and `intent_hash`.

**Open work**: Owner-side durable state saying that one effect intent is pending terminal
settlement.

**Owner**: The authoritative world side of execution. In the current implementation this is the
synchronous kernel plus the unified node scheduler that commits kernel output to the selected
journal backend.

**Effect runtime**: The async edge runtime that starts opened async effects after durable flush,
tracks only ephemeral execution state, and sends continuations back through world input.

**Adapter/executor**: Code that performs a concrete effect kind or adapter route, such as HTTP,
LLM, blob, timer, vault, or host/session work. Adapters are non-authoritative.

**Stream frame**: A non-terminal continuation for an open effect. Stream frames report progress or
partial output without settling the effect.

**Receipt**: The terminal continuation for an open effect. A receipt records the final observed
outcome and settles the effect when admitted by the owner.

## 4) Effect Classes

Not all effect kinds are executed the same way. The runtime class determines what happens after the
kernel opens work and the node durably flushes the frame.

### 4.1 Internal Deterministic Effects

Internal deterministic effects are handled on the owner side. They are still modeled as effects
because they need capability/policy checks, auditability, and a uniform receipt path, but they do
not leave deterministic execution.

Examples:

- `workspace.*`
- `introspect.*`
- in-world `governance.*`

Internal deterministic effects must not perform nondeterministic I/O. Their receipts are derived
from owner state and canonical data already available to the kernel/node.

### 4.2 Owner-Local Async Effects

Owner-local async effects are asynchronous but owned by the same node that owns the world. Timers
are the canonical example.

A timer is not a pure kernel operation because waiting for wall-clock time is nondeterministic. It
is also not a remote provider operation that needs a separate durable control plane. The owner opens
timer work durably, the owner-local scheduler waits until due time, and the due continuation
re-enters through normal owner admission.

Timer work remains open until a terminal receipt is admitted.

### 4.3 External Async Effects

External async effects interact with systems outside the owner:

- HTTP services,
- LLM providers,
- blob/object stores,
- vault/secret backends,
- host/session/file tools,
- future custom adapters.

The executor may maintain provider handles, attempts, task handles, streaming state, and retry
state. That state is operational cache. It is not authoritative world state.

## 5) End-to-End Lifecycle

### 5.1 Declaration

An effect kind must be known through the built-in catalog or manifest-declared `defeffect` entries.
The catalog supplies:

- parameter schema,
- receipt schema,
- capability type,
- allowed origin scope.

Workflow modules that may emit an effect kind must also declare it in
`abi.workflow.effects_emitted`. This structural allowlist is checked before capability and policy
authorization.

### 5.2 Emission

During a workflow step, a workflow module may return zero or more effect intents. These returned
intents are data, not side effects. At this point no external operation has started.

The kernel attaches origin identity to each emitted intent. Origin identity includes:

- workflow module identity,
- keyed instance identity when present,
- emitted sequence/position,
- effect kind,
- workflow-requested idempotency value when present.

### 5.3 Canonicalization

Before an intent is accepted, the kernel canonicalizes effect params:

1. Decode params against the effect kind's parameter schema.
2. Validate shape and type constraints.
3. Normalize values using AIR canonicalization rules.
4. Re-encode params as canonical CBOR.

Only canonical params participate in intent hashing, policy/capability checks, journal records, and
adapter dispatch. Authoring sugar and SDK convenience shapes must not perturb intent identity.

### 5.4 Authorization

An effect may proceed only when both gates pass:

1. Capability grant exists, has the correct capability type, has not expired, and permits the
   canonical params.
2. Policy allows the effect for the origin, effect kind, capability, and relevant metadata.

Denied effects fail deterministically at owner admission. They do not start external work.

### 5.5 Open Work

After canonicalization and authorization, the kernel records open work in deterministic owner
state. Open work includes enough information to:

- route future continuations to the origin workflow instance,
- identify the effect by `intent_hash`,
- preserve quiescence/apply safety,
- rebuild runtime execution cache after restart,
- explain the cause/effect chain during audit.

Open work is authoritative only after the world frame containing it is durably appended.

### 5.6 Durable Flush Publication Fence

Opened async effects MUST NOT be published to executors before the world frame that contains the
open work has durably flushed to the journal backend.

This is the main safety fence in the async effect system. It prevents the node from starting an
external operation for a speculative state transition that might later be rolled back. If a flush
fails, the node may discard staged slices and reopen the world from checkpoint/journal state, but no
executor has started the uncommitted opened effects from the failed slice.

Direct HTTP/control acceptance uses the same pipeline. A caller may return after enqueue/acceptance
or wait for flush, but both modes feed the same durable owner path.

### 5.7 Publication

After durable flush, the node publishes opened async effects to the effect runtime. Publication is
not itself authoritative. It is a way to tell ephemeral runtime machinery that durable open work now
exists and should be started or reconciled.

The effect runtime should treat publication as "ensure started" rather than
`execute(intent) -> receipt`. Some effects complete quickly, some stream, some need retries, and
some may already exist on a provider after restart.

### 5.8 Execution

Adapters execute outside the deterministic kernel. They may:

- start provider operations,
- retry transient failures,
- keep task handles,
- manage streaming connections,
- track provider-native operation IDs,
- translate provider responses into typed receipt payloads.

Adapters MUST NOT mutate world state directly. Their only path back into the world is through
continuations submitted to owner admission.

### 5.9 Continuation Admission

Stream frames and terminal receipts re-enter as world input. The owner validates:

- target world,
- `intent_hash`,
- open-work existence,
- continuation schema,
- per-effect stream sequence/fencing,
- terminal settlement rules.

Accepted continuations are canonicalized and journaled. Rejected continuations do not mutate world
state. Terminal receipts settle open work and resume the originating workflow instance through the
recorded origin identity, not through `routing.subscriptions`.

## 6) Intent Identity and Idempotency

`intent_hash` is the authoritative owner-side identity for one open effect.

For system/governance origins, the effective idempotency input is the explicit supplied key, or the
all-zero key when omitted.

For workflow-origin effects, `intent_hash` is not merely a hash of effect params. The kernel first
derives the effective idempotency input from stable origin identity and emission position. In
architecture terms, that derivation includes:

- origin workflow identity,
- origin instance key when keyed,
- effect kind,
- canonical params,
- workflow-requested idempotency value,
- effect index within the step,
- emitted sequence.

The final hash is computed over the effect kind, canonical params, capability grant name, and
effective idempotency input.

This makes `intent_hash` a per-emission open-work identity. It is used for:

- pending/open owner state,
- continuation routing,
- stream fencing,
- replay,
- quiescence,
- idempotent executor reconciliation.

Executors may maintain separate operational identities such as `attempt_id`, `operation_id`, or
provider-native handles. Those identities are not second owner-side effect IDs.

## 7) Stream Frames and Receipts

The coarse owner lifecycle is:

`open -> terminal`

Optional stream frames are observations on an open effect. They do not create a new authoritative
phase and do not settle the effect by themselves.

The recommended continuation shape is:

1. zero or more stream frames,
2. exactly one terminal receipt.

Receipts must be schema-bound to the effect kind's receipt schema. If receipt payload decoding or
normalization fails, the owner settles the effect through the workflow receipt-fault path described
in [spec/04-workflows.md](04-workflows.md).

## 8) Ordering and Concurrency

Distinct open effects may progress and settle out of emission order. This is required for practical
async execution: an HTTP request may complete before an earlier LLM stream, and timer continuations
may arrive while other work is still running.

The ordering rule is per effect:

- stream frames for one effect must satisfy that effect's sequence/fencing rules,
- a terminal receipt settles that effect once,
- duplicate or late terminal continuations are rejected or ignored according to owner admission
  rules,
- continuations for unknown or already-settled work do not advance authoritative state.

Workflow code must model business ordering explicitly in its own state. The runtime does not
pretend that concurrent external systems settle in workflow emission order.

## 9) Recovery

On restart, recovery proceeds from durable owner state:

1. Load checkpoint/snapshot baseline.
2. Replay journal frames.
3. Reconstruct open work and continuation routing.
4. Rebuild effect runtime cache from open work.
5. Ask executors to ensure or reconcile started work.

No second persisted effect-state database is required for correctness. Runtime caches such as
queues, task handles, stream maps, and provider handles are derived state.

Once admitted non-terminal evidence shows that external work exists, recovery is no longer deciding
whether stale work may begin from scratch. It is deciding how to reattach to, observe, or settle
already-existing work.

## 10) Quiescence and Governance

Open work is visible to diagnostics, governance, and strict quiescence.

Manifest apply is blocked while in-flight runtime work exists. The runtime does not implicitly
abandon, clear, or hide open effects during apply. A workflow that wants to stop waiting for work
must model cancellation, timeout, compensation, or terminal failure explicitly.

Shadow reports are bounded by the observed execution horizon. They can report observed effects,
pending work, workflow instance state, and ledger deltas. They do not promise complete static
prediction of future branches that have not executed.

## 11) Backpressure and Failure

The effect system must bound resource growth without changing deterministic semantics.

Implementations may apply backpressure at:

- workflow step output limits,
- per-world open-work limits,
- per-world staged-slice limits,
- effect-runtime queue limits,
- adapter-specific concurrency limits.

Backpressure must be reported as an acceptance or execution error that the owner can journal or
surface deterministically. It must not create hidden external work.

Flush failure and adapter failure are different:

- Flush failure means the world frame did not become durable. Opened async effects from that frame
  must not have been published.
- Adapter failure means durable open work exists, execution was attempted, and the failure must
  return through a terminal receipt or retry policy.

## 12) Relationship To Workflows

Workflow modules own:

- business state,
- retry and compensation decisions,
- deadlines and escalation policy,
- interpretation of receipt payloads.

The effect system owns:

- canonicalization,
- capability and policy gates,
- durable open-work tracking,
- post-flush async publication,
- continuation admission,
- replay/recovery invariants.

This split is what lets workflows orchestrate real external systems without giving workflow code
ambient authority or nondeterministic execution.
