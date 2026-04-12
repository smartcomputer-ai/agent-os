# P13: Hosted Timer Runtime and Tests

**Priority**: P13  
**Effort**: Medium/High  
**Risk if deferred**: High (timer effects may appear part of the hosted model while no real hosted
timer execution path exists)  
**Status**: Complete

## Goal

Add a real hosted timer execution path for the Kafka/S3 runtime and define the minimum timer tests
required once that path exists.

Before implementation, timer semantics existed in the shared log-first model:

- timer effects can be emitted by workflows
- hosted workers already knew how to translate timer receipt completions once they re-entered
  hosted ingress

But the hosted runtime does not yet have the actual timer-driving loop that decides when a timer is
due and re-injects it into hosted ingress.

## Current Gap

The embedded/local runtime has a timer runner and scheduler.

The hosted worker loop did not.

In practical terms, the hosted supervisor currently does:

1. sync Kafka assignments
2. drain submission batches
3. execute and commit frames
4. publish checkpoints

It does not currently:

1. track due timers across owned worlds
2. poll for the next due timer deadline
3. emit `TimerFired` submissions back into Kafka ingress
4. prove replay/restart semantics for timer-driven worlds

## Design Stance

Hosted timers should follow the same log-first rules as other external runtime work:

- timer due-ness is derived from authoritative durable world state
- timer firing enters through hosted submission flow, not an out-of-band local shortcut
- world state changes only when the resulting timer-fired event is committed through the
  authoritative world log
- restart and worker handoff must remain correct

The timer mechanism is runtime infrastructure, not a special second execution system.

## Minimum Runtime Work

### 1) Durable timer state source

The hosted worker must be able to determine, for the worlds it owns:

- whether a timer is pending
- when the next due timer should fire
- what payload must be submitted when it fires

The authoritative source for this must remain replayable world state, not ephemeral worker memory.

### 2) Due-timer scan / scheduling loop

Add hosted worker logic that:

1. inspects owned worlds for due timer work
2. submits a normal hosted timer receipt back to the correct routed world through ingress
3. uses the same route-epoch and ownership rules as other hosted submissions

This may start as a simple periodic scan. It does not need a sophisticated distributed timer wheel
for the first implementation.

### 3) Handoff / restart behavior

The hosted timer path must remain correct when:

- the worker restarts
- partition ownership changes
- a timer becomes due while no previous hot in-memory timer state survives

That means the new owner must reconstruct enough timer state from checkpoint + replay.

## Required Tests

### 1) Timer persists and fires in hosted mode

Add a real Kafka/MinIO worker test that proves:

1. a workflow emits `TIMER_SET`
2. the world reaches a waiting state
3. the hosted timer path emits a due timer firing
4. the worker processes `sys/TimerFired@1`
5. the world reaches the expected post-timer state

### 2) Timer survives restart

Add a hosted worker test that proves:

1. a timer is scheduled
2. the worker/runtime restarts before firing
3. the recovered worker still fires the timer once due
4. the world reaches the correct final state

### 3) Timer handoff survives ownership change

Add a hosted worker test that proves:

1. worker A owns the partition and schedules a timer
2. worker A stops before the timer fires
3. worker B takes ownership
4. worker B fires the timer and the world completes exactly once

## Explicit Non-Goals

- portal-send coverage
- rich timer service topology
- external timer microservice design
- long-term scheduling optimization
- advanced timer batching heuristics

The first goal is simply to make hosted timers real and correct.

## DoD

P13 is complete when:

1. Hosted workers have a real timer-driving path, not only a `TimerFired` payload shape.
2. A hosted Kafka/MinIO test proves timer set -> due -> fired -> world advance.
3. A hosted Kafka/MinIO test proves timer correctness across restart.
4. A hosted Kafka/MinIO test proves timer correctness across worker ownership handoff.
5. The roadmap clearly distinguishes timer runtime work from the broader worker-flow test gap in
   P12.
