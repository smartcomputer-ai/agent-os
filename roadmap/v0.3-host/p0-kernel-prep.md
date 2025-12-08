# P0: Kernel Hooks for Host Recovery

P1’s host/daemon needs a few kernel surfaces that don’t exist yet. Add these before or alongside P1 so restart safety and durable dispatch can be implemented without poking kernel internals:

- [x] **Tail scan helper**: given the last snapshot height, return journal entries (intents and receipts) after that point. Needed to requeue intents that were recorded but not yet snapshotted and to build the “receipts seen” set for de-dupe.
- [x] **Pending reducer receipt contexts**: expose the `pending_reducer_receipts` map (effect kind + params) so timer scheduling can be rebuilt on restart.
- [x] **Queued effects snapshot**: accessor for current in-memory `queued_effects` (already serialized into snapshots) to hydrate the dispatch queue on open.
- [x] **Plan pending receipts**: accessor for `pending_receipts` (plan_id + intent_hash) so hosts can avoid re-dispatching intents already awaited by plans.
- [x] **Structured snapshot/tail heights**: ensure callers can get the latest snapshot height and the journal head to decide whether a tail scan is needed.
- [x] **Journal head helper**: light-weight API to read current `next_seq` without loading the full log (used by control-server `journal-head`/health checks).

These are kernel-only changes; the host will call them to implement the durable outbox/rehydration flow described in P1.
