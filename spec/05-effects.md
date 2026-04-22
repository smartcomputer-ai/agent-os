# Effects And Async Execution

Effects are the boundary between deterministic world execution and everything that can observe or
change the outside world.

The kernel must be replay-identical: given the same manifest, checkpoint/snapshot, journal frames,
and receipts, it reconstructs the same state. Network calls, timers, LLM providers, host file tools,
blob services, and secret stores do not have that property.

AgentOS handles this by making external work explicit. Workflows request work by emitting typed
effect intents. The owner records that work durably before anything outside the world may start.
Async executors perform the work outside the deterministic kernel and return stream frames or
terminal receipts. Those continuations re-enter as ordinary world input and are admitted only by the
owner.

## 1) Scope

This spec covers:

- effect declarations and runtime classes
- effect intent canonicalization and admission
- durable open work
- the post-flush publication fence
- async effect runtime and adapter responsibilities
- stream frame and receipt admission
- intent identity, idempotency, ordering, recovery, and quiescence

It does not define:

- the full built-in op catalog; see [spec/03-air.md](03-air.md)
- workflow state-machine patterns; see [spec/04-workflows.md](04-workflows.md)
- concrete queue, task, or deployment topology
- provider-specific retry, timeout, billing, or API contracts

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

External work is never hidden behind a normal function call. Workflow code does not "call HTTP"; it
emits a request for `sys/http.request@1`. The receipt is later delivered as input.

## 3) Vocabulary

**Effect**: A `defeffect` definition. It names parameter and receipt schemas and an implementation
module/entrypoint.

**Effect intent**: A canonical request to perform one effect. It includes effect identity,
canonical params, origin metadata, idempotency input, and `intent_hash`.

**Open work**: Owner-side durable state saying that one effect intent is pending terminal
settlement.

**Owner**: The authoritative world side of execution. In the current implementation this is the
synchronous kernel plus the unified node scheduler that commits kernel output to the selected
journal backend.

**Effect runtime**: The async edge runtime that starts opened async effects after durable flush,
tracks only ephemeral execution state, and sends continuations back through world input.

**Adapter/executor**: Code that performs a concrete effect route, such as HTTP, LLM, blob, timer,
vault, or host/session work. Adapters are non-authoritative.

**Stream frame**: A non-terminal continuation for an open effect. Stream frames report progress or
partial output without settling the effect.

**Receipt**: The terminal continuation for an open effect. A receipt records the final observed
outcome and settles the effect when admitted by the owner.

## 4) Effect Runtime Classes

Not all effects execute the same way. The runtime class determines what happens after the kernel
opens work and the node durably flushes the frame.

### 4.1 Internal deterministic effects

Internal deterministic effects are handled on the owner side. They are still modeled as effects
because they need declaration checks, auditability, and a uniform receipt path, but they do not leave
deterministic execution.

Examples:

- `sys/workspace.*@1`
- `sys/introspect.*@1`
- in-world `sys/governance.*@1`

Internal deterministic effects must not perform nondeterministic I/O. Their receipts are derived
from owner state and canonical data already available to the kernel/node.

### 4.2 Owner-local async effects

Owner-local async effects are asynchronous but owned by the same node that owns the world. Timers
are the canonical example.

A timer is not a pure kernel operation because waiting for wall-clock time is nondeterministic. The
owner opens timer work durably, the owner-local scheduler waits until due time, and the due
continuation re-enters through normal owner admission.

### 4.3 External async effects

External async effects interact with systems outside the owner:

- HTTP services
- LLM providers
- blob/object stores
- vault/secret backends
- host/session/file tools
- future custom adapters

The executor may maintain provider handles, attempts, task handles, streaming state, and retry
state. That state is operational cache. It is not authoritative world state.

## 5) End-To-End Lifecycle

### 5.1 Declaration

An effect is declared as `defeffect`. Built-in `sys/*` effects are ambiently available; user effects
are listed in `manifest.effects`. The effect definition
supplies:

- parameter schema
- receipt schema
- implementation module
- implementation entrypoint

Workflows that may emit an effect must list that effect in `effects_emitted`. This structural
allowlist is checked before open work is recorded.

### 5.2 Emission

During a workflow step, a workflow may return zero or more effect intents. These returned intents
are data, not side effects. At this point no external operation has started.

The kernel attaches origin identity to each emitted intent. Origin identity includes:

- workflow identity
- workflow hash when available
- keyed instance identity when present
- emitted sequence/position
- workflow-requested idempotency value when present

### 5.3 Canonicalization

Before an intent is accepted, the kernel canonicalizes effect params:

1. Resolve the effect from active definitions or the ambient built-in catalog.
2. Decode params against the effect's parameter schema.
3. Validate shape and type constraints.
4. Normalize values using AIR canonicalization rules.
5. Re-encode params as canonical CBOR.

Only canonical params participate in intent hashing, journal records, and adapter dispatch.
Authoring sugar and SDK convenience shapes must not perturb intent identity.

### 5.4 Admission

The public AIR v2 surface has no caps, cap grants, or policy language. Effect admission is
structural. An effect may proceed only when all of these checks pass:

1. The effect exists in active definitions or the ambient built-in catalog.
2. Params validate against the effect params schema and canonicalize successfully.
3. Workflow-origin effects come from a workflow.
4. The effect is listed in the origin workflow's `effects_emitted`.

Rejected effects fail deterministically at owner admission. They do not start external work. Hosted
deployments can add admission policy outside public AIR.

### 5.5 Open work

After canonicalization and admission, the kernel records open work in deterministic owner state.
Open work includes enough information to:

- route future continuations to the origin workflow instance
- identify the effect by `intent_hash`
- bind the effect identity and resolved effect hash
- preserve quiescence/apply safety
- rebuild runtime execution cache after restart
- explain the cause/effect chain during audit

Open work is authoritative only after the world frame containing it is durably appended.

### 5.6 Durable flush publication fence

Opened async effects MUST NOT be published to executors before the world frame that contains the
open work has durably flushed to the journal backend.

This prevents the node from starting an external operation for a speculative state transition that
might later be rolled back. If a flush fails, the node may discard staged slices and reopen the world
from checkpoint/journal state, but no executor has started the uncommitted opened effects from the
failed slice.

Direct HTTP/control acceptance uses the same pipeline. A caller may return after enqueue/acceptance
or wait for flush, but both modes feed the same durable owner path.

### 5.7 Publication

After durable flush, the node publishes opened async effects to the effect runtime. Publication is
not itself authoritative. It tells ephemeral runtime machinery that durable open work exists and
should be started or reconciled.

The effect runtime should treat publication as "ensure started" rather than
`execute(intent) -> receipt`.

### 5.8 Execution

Adapters execute outside the deterministic kernel. They may:

- start provider operations
- retry transient failures
- keep task handles
- manage streaming connections
- track provider-native operation IDs
- translate provider responses into typed receipt payloads

Adapters MUST NOT mutate world state directly. Their only path back into the world is through
continuations submitted to owner admission.

### 5.9 Continuation admission

Stream frames and terminal receipts re-enter as world input. The owner validates:

- target world
- `intent_hash`
- open-work existence
- effect identity and hash when present
- continuation schema
- per-effect stream sequence/fencing
- terminal settlement rules

Accepted continuations are canonicalized and journaled. Rejected continuations do not mutate world
state. Terminal receipts settle open work and resume the originating workflow instance through the
recorded origin identity, not through `routing.subscriptions`.

## 6) Intent Identity And Idempotency

`intent_hash` is the authoritative owner-side identity for one open effect.

For system/governance origins, the effective idempotency input is the explicit supplied key, or the
all-zero key when omitted.

For workflow-origin effects, `intent_hash` is not merely a hash of effect params. The kernel first
derives the effective idempotency input from stable origin identity and emission position:

- origin workflow
- origin workflow hash when available
- origin instance key when keyed
- effect
- effect hash when available
- canonical params
- workflow-requested idempotency value
- effect index within the step
- emitted sequence

The final hash is computed over effect identity, canonical params, and effective idempotency
input. This makes `intent_hash` a per-emission open-work identity.

Executors may maintain separate operational identities such as `attempt_id`, `operation_id`, route
labels, or provider-native handles. Those identities are not second owner-side effect IDs.

## 7) Stream Frames And Receipts

The coarse owner lifecycle is:

```text
open -> terminal
```

Optional stream frames are observations on an open effect. They do not create a new authoritative
phase and do not settle the effect by themselves.

The recommended continuation shape is:

1. zero or more stream frames
2. exactly one terminal receipt

Receipts must be schema-bound to the effect's receipt schema. If receipt payload decoding or
normalization fails, the owner settles the effect through the workflow receipt-fault path described
in [spec/04-workflows.md](04-workflows.md).

Generic receipt and stream envelopes carry:

- origin workflow identity and optional hash
- origin instance key when keyed
- intent hash identity
- effect identity and optional hash
- executor module, executor module hash, and executor entrypoint when resolved
- params hash, issuer ref, payload bytes or payload ref, status/sequence data, cost, and signature

## 8) Ordering And Concurrency

Distinct open effects may progress and settle out of emission order. This is required for practical
async execution.

The ordering rule is per effect:

- stream frames for one effect must satisfy that effect's sequence/fencing rules
- a terminal receipt settles that effect once
- duplicate or late terminal continuations are rejected or ignored according to owner admission
  rules
- continuations for unknown or already-settled work do not advance authoritative state

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

## 10) Quiescence And Governance

Open work is visible to diagnostics, governance, and strict quiescence.

Manifest apply is blocked while in-flight runtime work exists. The runtime does not implicitly
abandon, clear, or hide open effects during apply. A workflow that wants to stop waiting for work
must model cancellation, timeout, compensation, or terminal failure explicitly.

Shadow reports are bounded by the observed execution horizon. They can report observed effects,
pending work, workflow instance state, and state deltas. They do not promise complete static
prediction of future branches that have not executed.

## 11) Backpressure And Failure

The effect system must bound resource growth without changing deterministic semantics.

Implementations may apply backpressure at:

- workflow step output limits
- per-world open-work limits
- per-world staged-slice limits
- effect-runtime queue limits
- adapter-specific concurrency limits

Backpressure must be reported as an acceptance or execution error that the owner can journal or
surface deterministically. It must not create hidden external work.

Flush failure and adapter failure are different:

- Flush failure means the world frame did not become durable. Opened async effects from that frame
  must not have been published.
- Adapter failure means durable open work exists, execution was attempted, and the failure must
  return through a terminal receipt or retry path.

## 12) Relationship To Workflows

Workflows own:

- business state
- retry and compensation decisions
- deadlines and escalation behavior
- interpretation of receipt payloads

The effect system owns:

- canonicalization
- structural effect admission
- durable open-work tracking
- post-flush async publication
- continuation admission
- replay/recovery invariants

This split lets workflows orchestrate real external systems without giving workflow code ambient
authority or nondeterministic execution.
