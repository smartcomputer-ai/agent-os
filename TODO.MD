## Upcoming Kernel/AIR Tasks

1. **Keyed Routing Pipeline**
   - Detected gap: manifest `routing.events[].key_field` is still ignored at runtime, so keyed reducers can’t receive events even though plans can now emit `DomainEvent.key` bytes.
   - Why it matters: Spec 05 (Cells) expects us to fan events to `(reducer, key)` and eventually persist per-cell state; without this plumbing, keyed reducers remain theoretical and plans can’t coordinate with stateful cells.

2. **Plan Result Schema Enforcement**
   - Detected gap: `PlanStep::End` enforces presence/absence of a result but never validates the returned value against the plan’s declared output schema.
   - Why it matters: governance/shadow runs rely on typed outputs to diff manifests and produce receipts; accepting malformed results undermines deterministic replay and AIR guarantees in spec 03 §11.

3. **Expose Plan Results to Callers**
   - Detected gap: `PlanTickOutcome.result` is computed but never surfaced—no journal record, no CLI inspection, no governance summary.
   - Why it matters: operators and downstream reducers/plans need visibility into plan outputs for audit/shadow replay; persisting the result (and referencing it from governance/shadow reports) completes the orchestration loop promised in spec 02.

4. **Invariants in Governance & Testkit**
   - Detected gap: plan invariants now fail fast inside the runtime, but governance shadow runs and `aos-testkit` scenarios don’t report or assert on those failures.
   - Why it matters: reviewers must understand invariant violations during proposal review, and integration suites should prove invariants stay satisfied across replays; wiring this into the shadow summary and adding fixture coverage keeps invariants actionable instead of silent aborts.

5. **Keyed Reducer Storage & Replay**
   - Detected gap: even with keyed events, reducer state remains monolithic; there’s no per-cell storage/journal path described in spec 05 (Cells) §“Storage Layout and Snapshots”.
   - Why it matters: shipping cell-mode requires CAS-backed per-key state files, snapshot inclusion, journal metadata, and replay hooks, otherwise keyed reducers can’t scale or survive restarts.
