# 20-agent-session

Session lifecycle smoke fixture for `aos-agent-sdk` conformance and replay parity:
- deterministic tool fan-in/fan-out and cancellation fences,
- loop-cap circuit breaker behavior,
- unknown provider/model start rejection without partial activation,
- run config immutability within a run and provider/model updates across runs.
