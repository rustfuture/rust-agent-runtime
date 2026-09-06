# Release notes

## 0.1.0 — runtime core candidate

- Durable append-only task states and attempt counts.
- Idempotent enqueue and restart recovery.
- Deterministic queued-task selection.
- Allowlisted, workspace-contained process execution.
- Timeout, cancellation, and bounded stdout/stderr capture.
- Persisted succeeded, failed, and cancelled outcomes.
- Durable metadata-only command traces and restart reconciliation.
- Explicit bounded retry for failed tasks.

No LLM provider, tool planner, patch generator, network isolation, process-tree containment, UI, or automatic merge is included in this candidate. Exactly-once external side effects are not guaranteed if the process dies before a trace is synced; adapters must supply idempotency or transactions.
