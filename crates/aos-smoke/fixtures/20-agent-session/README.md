# 20-agent-session

Session lifecycle smoke fixture for `aos-agent-sdk` conformance and replay parity:
- AIR `aos.agent/*` schemas are imported from `crates/aos-agent-sdk/air`,
- workflow runtime wasm is compiled from `aos-agent-sdk` bin `session_workflow`,
- routing binds directly to keyed SDK module `aos.agent/SessionWorkflow@1`,
- deterministic tool fan-in/fan-out and cancellation fences,
- loop-cap circuit breaker behavior,
- unknown provider/model start rejection without partial activation,
- run config immutability within a run and provider/model updates across runs.
