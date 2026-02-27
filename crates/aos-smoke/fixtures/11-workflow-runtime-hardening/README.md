# 11-workflow-runtime-hardening

Single-scenario workflow-runtime hardening fixture covering high-value runtime invariants:

- correlation-safe gating (`Start` + `Approval` by `request_id` in workflow state),
- concurrent request isolation (no cross-talk between request IDs),
- crash/resume while workflow instances wait on external HTTP receipts,
- deterministic replay parity for state and workflow observability summary,
- workflow summary artifact generation (`workflow-summary.json`).

The scenario runs two concurrent requests, approves one first to verify isolation,
restarts during in-flight worker receipts, then completes both requests and verifies
replay + summary invariants.
