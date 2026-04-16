# Journal Backend Matrix

**Status**: Working note  
**Context**: follows the P2 decision to remove Kafka ingress from hosted runtime architecture while
keeping the journal backend decision open

## What We Need From A Journal

For hosted AgentOS, the journal backend should support:

1. append-only durable recording of world frames and related durable dispositions,
2. clean replay/bootstrap semantics,
3. strong per-world ordering,
4. an honest durable commit point for optional wait-until-flushed behavior,
5. operationally understandable failure and recovery behavior,
6. future support for explicit ownership and later pluggable discovery/checkpoint work.

Nice-to-haves:

1. external subscriber/fan-out support,
2. multi-region or HA operational patterns,
3. manageable cost and ops burden,
4. room for future scale without forcing Kafka-shaped execution semantics.

## Recommendation

Current recommendation:

1. keep Kafka journal only as a transitional backend while ingress is removed and ownership is
   simplified,
2. seriously evaluate `PostgreSQL` as the default next backend,
3. evaluate `FoundationDB` if long-term distributed transactional scale is a primary goal,
4. consider `EventStoreDB/KurrentDB` if the product should adopt an explicit event-store stance.

## Matrix

| Backend | Fit For AOS Journal | Strengths | Weaknesses | Recommendation |
| --- | --- | --- | --- | --- |
| `PostgreSQL` | Strong | Operationally familiar, mature HA/backup story, simple commit model, clean explicit ownership model, good tooling, easy to reason about replay/checkpoints | Not a native streaming backbone, horizontal scale story is less natural than Kafka/FoundationDB, logical replication is not a perfect subscriber model | Best default if the priority is clarity and shipping |
| `FoundationDB` | Very strong | Strict serializable ACID transactions, strong durability, clean model for journal plus indexes plus ownership metadata, strong long-term architecture base | Higher implementation and operational complexity, smaller talent pool, requires careful data-model design | Best long-term distributed substrate if complexity is acceptable |
| `EventStoreDB / KurrentDB` | Strong | Native event-store model, append-only streams, optimistic concurrency, idempotent append semantics, commit position maps well to wait-until-flushed | More specialized product choice, less general than Postgres/FoundationDB for mixed metadata/index workloads | Strong option if AgentOS wants first-class event-store identity |
| `Kafka / Redpanda` | Medium | High throughput, strong fan-out, mature append-log model, existing code already uses it, Redpanda improves ops story | Architecture pressure toward partition/consumer-group thinking, transactions are shaped around source offsets and stream-processing patterns, more infrastructure than needed if journal is mainly internal | Keep only as transitional backend unless stream fan-out is core product value |
| `Kinesis` | Medium-low | Managed AWS service, durable streaming, easy AWS adoption | Still shard/checkpoint shaped, weaker as a canonical internal journal, retention-oriented not long-term database-oriented, AWS lock-in | Only if AWS-native streaming is a hard requirement |
| `Pulsar` | Medium-low | Supports transactions, multi-topic semantics, streaming platform features | Another complex streaming system with coordinator semantics, not obviously simpler than Kafka for this use case | Not a first-choice canonical journal |
| `NATS JetStream` | Medium-low | Simpler than Kafka to start, useful messaging platform, dedupe-based exactly-once story | Journal semantics are weaker/less database-like, exactly-once story is narrower, not ideal as canonical event-sourced substrate | Better as messaging infrastructure than canonical journal |
| `SQLite` | Medium for local/embedded, low for enterprise hosted | Extremely simple, deterministic, excellent local/dev backend, already aligned with embedded use | Single-node story, limited hosted HA/concurrency scaling, not the right enterprise default by itself | Strong embedded/local backend, not primary hosted enterprise journal |

## Decision Notes

### PostgreSQL

Why it fits:

1. easiest serious system to understand and operate,
2. explicit ownership and direct HTTP ingress fit naturally,
3. journal append plus checkpoint/discovery tables can live in one transactional substrate,
4. easier to reason about than a stream processor disguised as a database.

Main risk:

1. if very high-scale fan-out streaming becomes a core product requirement, Postgres may stop being
   the right center.

### FoundationDB

Why it fits:

1. best clean-room architecture if the goal is a durable transactional substrate for worlds,
2. one system can hold journal records, ownership state, checkpoint metadata, and discovery state,
3. strong correctness story for future multi-worker coordination.

Main risk:

1. it raises the implementation bar substantially right away.

### EventStoreDB / KurrentDB

Why it fits:

1. closest semantic match to an append-only world journal,
2. strong stream identity and optimistic concurrency,
3. commit position is a natural durable acceptance fence.

Main risk:

1. it is a narrower platform choice and may still need adjacent storage/index structures around it.

### Kafka / Redpanda

Why it fit the old architecture:

1. ingress and journal were both organized around the same partition model,
2. exactly-once flow was tied to consuming source partitions and committing offsets transactionally.

Why it fits less well after P2:

1. worker ownership is no longer supposed to come from ingress partition assignment,
2. direct HTTP acceptance is the new center,
3. Kafka remains useful mainly as a durable log, not as the shape of execution.

## Practical Path

Recommended order:

1. finish P2 with direct HTTP ingress and explicit ownership,
2. keep the current Kafka journal only long enough to stabilize that model,
3. prototype a `PostgreSQL` journal backend first,
4. evaluate whether `FoundationDB` is worth the extra complexity for the next stage,
5. keep `SQLite` as the embedded/local backend regardless of hosted choice.

## Short Conclusion

If the goal is the clearest enterprise hosted architecture:

1. `PostgreSQL` is the best next backend to try,
2. `FoundationDB` is the strongest long-term option if the team wants to pay the complexity cost,
3. `Kafka` should probably stop being the architectural center once ingress is removed from the
   model.
