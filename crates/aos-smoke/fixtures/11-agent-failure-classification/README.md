# 11-agent-failure-classification

Agent SDK conformance fixture for trace-based failure diagnosis:
- `policy_denied`,
- `capability_denied`,
- `adapter_timeout`,
- `adapter_error`.

This fixture validates failure classification through existing trace surfaces:
- `trace-get` query construction/output shape,
- `trace-diagnose` cause mapping logic (shared implementation).
