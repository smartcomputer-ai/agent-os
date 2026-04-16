# P1: Fabric Host Execution Objects And Control Surfaces

**Priority**: P1  
**Effort**: Very High  
**Risk if deferred**: High (host execution will remain an afterthought rather than a first-class
detached execution subsystem)  
**Status**: Proposed  
**Depends on**:

- `roadmap/v0.18-execution/p1-open-effect-lifecycle-and-owner-executor-seam.md`
- `roadmap/v0.18-execution/p2-start-eligibility-expiry-and-staleness.md`
- `roadmap/v0.18-execution/p3-runtime-execution-strategies.md`

## Goal

Define the fabric-specific execution layer that sits on top of the core owner/executor seam:

- hosted detached executors,
- host/container/vm/sandbox execution objects,
- sessions,
- artifact and log refs,
- secrets integration.

This document assumes the core lifecycle and restart rules already exist.
It does not redefine the open-effect seam.

## Core Design Stance

Host execution is important enough to be treated as a first-class fabric subsystem, not as an
anonymous generic adapter detail.

Worlds decide:

- what should happen,
- what semantic result they are waiting for,
- what execution class they need.

Fabric decides:

- where it runs,
- how it is provisioned,
- how sessions are managed,
- where logs and artifacts live,
- how detached executors reconcile and operate at scale.

## Fabric As A Consumer Of The Seam

Fabric should consume the `v0.18-execution` seam rather than redefining it.

Important rule:

- the world log remains the only authoritative state-advancing log,
- fabric workers submit continuations through ingress,
- partition owners admit those continuations into authoritative history,
- fabric transport chatter is not copied into the journal one-for-one.

## Minimal Plane Strategy

The correctness minimum remains:

- `aos-ingress` for non-authoritative submissions,
- `aos-journal` for authoritative world history,
- `aos-route` for compacted route metadata.

The design may additionally use detached execution lanes such as:

- `aos-effect`,
- `aos-fabric`.

Those lanes are operationally useful, but not authoritative world history.

## Required Fabric Concepts

The fabric layer should introduce first-class concepts such as:

- `execution_class`,
- `host_session`,
- `operation_id`,
- `artifact_ref`,
- `log_ref`.

These concepts belong to the detached execution and control surface layer, not the core workflow
runtime contract.

## Example Host Intents

- `SessionOpen`
- `WorkspaceMaterialize`
- `ExecRun`
- `ArtifactCollect`
- `ServiceDeploy`
- `ProbeWatch`
- `SessionClose`

## Example Host Continuations And Receipts

- `Accepted`
- `Started`
- `Progress`
- `Completed`
- `Failed`
- `LogReady`
- `ArtifactReady`
- `DeploymentStatus`

These describe the semantic space that fabric-backed execution may need.
They are not a requirement that every transport callback or worker protocol message be made
canonical world history.

## Session Stance

For dark-factory software workflows, warm execution environments matter.

Therefore:

- one-shot command execution is not the only useful model,
- host sessions should be first-class external objects,
- host sessions should not rely on long-lived attachment to a partition owner.

Sessions let agents reuse:

- cloned repositories,
- dependency caches,
- build artifacts,
- deployment handles,
- monitoring sessions.

The world stores references and semantic progress.
Fabric owns the actual external session.

## Artifact And Log Handling

Kafka should not be used as the bulk sink for noisy host output.

Recommended split:

- semantic lifecycle events and refs on Kafka,
- bulk bytes in S3 or another blob/object store.

Examples:

- stdout/stderr archives,
- test reports,
- screenshots,
- tarballs,
- deployment bundles.

## Secrets

Fabric should integrate with external secret systems rather than define an AOS-owned vault.

Required stance:

- world state stores only refs, names, and version metadata,
- workers resolve real secret values at execution time,
- plaintext secret values must not be carried in Kafka records.

Possible providers:

- Kubernetes secrets,
- cloud secret managers,
- vault products,
- workload-identity-attached providers.

## Idempotency And Failure Semantics

Host execution and many external tools are not truly exactly-once.

Therefore:

- every detached operation needs a stable `operation_id` or equivalent effect-instance key,
- fabric should use that identity for reconciliation and idempotency when possible,
- retries are driven by open work and continuations, not hidden mutable queue state.

## Scope

### [ ] 1) Define hosted detached execution surfaces

Required outcome:

1. fabric workers and detached execution lanes are first-class consumers of the owner/executor
   seam,
2. ingress and journal boundaries stay explicit.

### [ ] 2) Define first-class host/session objects

Required outcome:

1. sessions, execution classes, logs, and artifacts are explicit design concepts,
2. the world stores refs while fabric owns the external objects.

### [ ] 3) Define log and artifact handling

Required outcome:

1. Kafka carries semantic lifecycle records and refs,
2. blob/object storage carries bulk bytes.

### [ ] 4) Define secrets stance for fabric-backed execution

Required outcome:

1. fabric resolves secrets from external providers,
2. plaintext secret material does not become transport or world state.

## Non-Goals

This phase does **not** attempt:

1. to redefine the core owner/executor seam,
2. to redefine stale-start semantics,
3. to build a first-party secret vault product,
4. to guarantee exactly-once for arbitrary external effects,
5. to define final placement optimization policy for every future substrate.

## DoD

1. Fabric is positioned explicitly as a consumer of the `v0.18-execution` seam.
2. Detached execution lanes are optional operational planes, not authoritative world history.
3. Host execution is recognized as a first-class fabric subsystem.
4. Sessions, artifacts, and logs are explicit fabric design concepts.
5. Secret handling is defined in terms of external secret providers and refs.
