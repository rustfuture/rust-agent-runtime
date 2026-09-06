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
- Provider-neutral structured decisions and a step-bounded agent loop.
- AGY/Gemini 3.8 Flash Low adapter with usage/latency metadata and a real smoke example.
- Bounded workspace read/exact-replacement actions and a real failing-to-passing Rust fixture evaluation.
- Opt-in macOS Seatbelt execution that denies network and writes outside the workspace, covered by a real subprocess test.
- Terminal enqueue/cancel/status/watch interface with non-mutating observation and stricter terminal-state transitions.
- Real AGY NDJSON response streaming with callback delivery and final usage metadata.
- Post-edit verification debt that prevents an untested model edit from being reported complete.

No Linux isolation backend, process-tree containment, graphical UI, or automatic merge is included in this candidate. Exactly-once external side effects are not guaranteed if the process dies before a trace is synced; adapters must supply idempotency or transactions. Live AGY calls are not part of CI.
