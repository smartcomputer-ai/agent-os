# 10-trace-failure-classification

Trace/observability conformance fixture for workflow failure diagnosis:
- `policy_denied`,
- `capability_denied`,
- `adapter_timeout`,
- `adapter_error`.

This fixture validates failure classification through trace surfaces:
- `trace-get` query construction/output shape,
- `trace-diagnose` cause mapping logic (shared implementation).
