# Rust Agent Runtime

A durable task-state baseline for a bounded coding-agent runtime.

The first milestone intentionally contains no LLM and no arbitrary shell execution. It provides append-only task events, idempotent enqueue, attempts, cancellation, and restart recovery that requeues interrupted running tasks.

```bash
cargo test --locked
```

The bounded executor rejects program paths and non-allowlisted programs, canonicalizes the working directory under the configured workspace, kills timed-out children, disables stdin, and truncates captured stdout/stderr at a configured byte limit. These controls are not an OS sandbox and do not yet isolate network access or child-process trees.

`Runtime::run_next` deterministically selects the lowest queued task id, persists `running`, executes through the bounded executor, and records `succeeded` only for a zero exit status without timeout; all other outcomes become `failed`.

A clonable cancellation token is checked while a child runs. Cancellation kills the child, marks the execution as cancelled, and persists the task’s `cancelled` state.

See `docs/architecture.md` for the trust boundary and `RELEASE_NOTES.md` for the current candidate scope. Licensed under MIT.
