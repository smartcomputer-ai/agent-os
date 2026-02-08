Agreed. Keep kernel generic.

Then the right split is:

Kernel-level generic debug primitive
turn trace by event lineage, not chat semantics.
Inputs: --from-event-hash or --correlation-key name=value.
Output: DAG of DomainEvent -> Intent -> CapDecision/PolicyDecision -> Receipt -> RaisedEvent.
No chat_id, no app names, no Demiurge logic.
App-level adapter in shell/CLI
Demiurge maps chat_id/request_id to the underlying generic correlation:
find UserMessage event hash
call kernel trace primitive
render a chat-friendly view.
Minimal kernel utilities to add:

GET /api/debug/trace?event_hash=...
optional GET /api/debug/trace?correlate_by=request_id&value=2&schema=demiurge/ChatRequest@1
include:
event hash/schema
emitted intents (hash/kind/origin)
cap/policy decisions
receipts (status/adapter/error payload summary)
raised events
terminal state (completed, waiting_receipt, waiting_event, failed)
This keeps kernel app-agnostic and still gives exactly what we need to debug stalls.

If you want, I’ll draft the concrete response schema next so we can implement it without rework.