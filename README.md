# Rust Agent Runtime

A durable task-state baseline for a bounded coding-agent runtime.

The first milestone intentionally contains no LLM and no arbitrary shell execution. It provides append-only task events, idempotent enqueue, attempts, cancellation, and restart recovery that requeues interrupted running tasks.

```bash
cargo test --locked
```

Next: a bounded process executor with explicit command allowlists, timeouts, output limits, and workspace containment. A temporary directory alone is not treated as a sandbox.
