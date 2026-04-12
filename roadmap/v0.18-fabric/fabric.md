# v0.18 Fabric Follow-On

## Background

`v0.17-kafka` established the log-first hosted seam, Kafka/S3 recovery model, route-first hosted
runtime, and embedded/local convergence work. What it intentionally does not finish is the broader
fabric-side execution model:

- dedicated `aos-effect` / `aos-fabric` lanes
- hosted session and host-execution control surfaces
- artifact/log refs and control-plane semantics
- fabric-side secret-provider cutover

Those items are follow-on work. They are no longer treated as open scope inside
`roadmap/v0.17-kafka/`.

## Milestone Map

- `p1-effects-fabric-and-host-execution.md`
  - carry forward the deferred effects/fabric/host-execution/session/artifact/secrets design work

## Scope

This milestone is where we finish the subordinate execution plane around the already-landed
log-first world runtime:

- execution lanes remain subordinate to authoritative world history
- receipts still re-enter through authoritative owner admission
- session/artifact/log handling becomes explicit product/runtime surface area
- secret-provider cutover is handled as fabric execution concern rather than `v0.17` core hosted
  reset scope
