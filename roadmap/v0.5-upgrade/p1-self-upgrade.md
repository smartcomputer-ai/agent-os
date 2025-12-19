# P1: Self‑Upgrade via Governed Plans

**Priority**: P1  
**Effort**: Medium  
**Risk if deferred**: High (blocks agent‑led upgrades; governance remains operator‑only)

## What’s missing
- Governance verbs are only accessible out‑of‑band (CLI/control) and cannot be invoked through plans; reducers/plans cannot drive the `propose → shadow → approve → apply` loop.
- No governance effect kinds, schemas, or capability type; policy cannot gate “who may change the manifest”.
- No receipts or journal coupling for governance effects; replay relies on external tooling instead of typed intents/receipts.
- No plan pattern or manifest triggers for upgrade requests initiated in‑world (e.g., reducers emitting “upgrade me” intents).

## Why it matters
- Enables governed self‑modification: worlds can propose, rehearse, and apply their own manifest patches under explicit caps/policy, keeping homoiconicity and auditability.
- Reduces operational friction: the same path works for human‑initiated and agent‑initiated upgrades, with the audit trail in the journal.
- Keeps least‑privilege intact: governance actions become explicit effects that policy can allow/deny, rather than implicit operator power.

## Proposed work
1) **Governance effect catalog**  
   - Add `defeffect` entries: `governance.propose`, `governance.shadow`, `governance.approve`, `governance.apply` with plan‑only `origin_scope`.  
   - Define param/receipt schemas (canonical AIR) carrying `patch_cbor`, `proposal_id`, `manifest_hash_base/new`, `shadow_report`, `decision/approver`, and status.  
   - Add built‑in capability type `governance` and cap grants (e.g., `sys/govern@1`) that guard these effects.
   - TODO: embed `sys/governance@1` cap and a default policy stub (moved from P5).

2) **Policy wiring**  
   - Extend policy matching for `effect_kind=governance.*`; default deny.  
   - Provide templates for human‑in‑the‑loop approval (e.g., policy requires `approver` field or a specific principal) and for automated paths (caps scoped to certain modules/plans).

3) **Kernel + effect handler**  
   - Implement governance effect handlers that execute existing propose/shadow/approve/apply logic, append standard governance journal entries, and return typed receipts.  
   - Ensure deterministic replay consumes recorded governance receipts instead of re‑running validation; idempotency keys fence duplicate submissions.  
   - Reject attempts if sequencing invalid (apply without approved, mismatched hashes, etc.).

4) **Plan surface + triggers**  
   - Introduce system schemas for governance intents (e.g., `sys/GovActionRequested@1`) so reducers can emit requests; add manifest triggers to start privileged upgrade plans.  
   - Document plan patterns: reducer emits intent → upgrade plan performs propose/shadow → optional human gate → approve/apply → raises result event back to reducer.

5) **CLI/control coherence**  
   - Expose governance verbs in control channel as first‑class calls (mirroring p5 governance work), keeping the same validation path as the new effects.  
   - CLI should be able to drive or inspect the same proposals produced by in‑world plans; receipts/journal are the single source of truth.

6) **Tests/fixtures**  
   - Integration tests: full in‑world loop (emit intent → plan emits governance effects → receipts → replay).  
   - Negative cases: policy deny, cap missing, sequencing errors, mismatched hashes, duplicate apply.  
   - Golden journals to prove replay determinism and receipt binding.

## Design notes
- Keep reducers pure: reducers should request upgrades via DomainIntent; governance effects stay plan‑only to preserve orchestration/policy choke points.  
- Receipts should summarize manifest hashes and decisions so observability/audit can rely on journal alone.  
- Capabilities are the safety lever: small, scoped `govern` grants to specific upgrade plans/modules; default world policy denies.  
- Human approval remains possible: encode approver identity in params/receipts; policy can require it before `approve`/`apply`.  
- Out of scope: cross‑world orchestration and multi‑world policy delegation (leave to later roadmap).

## Deferred items (align with host P5 status)
- Governance effect adapter: intercept `governance.*` intents in-kernel, run the same propose/shadow/approve/apply handler, and emit receipts. Needed for plan-driven self-upgrade; deferred until p1 starts. 
- CLI helper `aos world gov propose --patch-dir <air>`: build patch from AIR bundle, fill hashes, validate, submit via control; preserves hashless authoring ergonomics.
