# Architecture

```text
enqueue -> append event -> queued -> running -> bounded executor
   ^                                      |           |
   |                                      |           +-> timeout / cancel / output cap
restart recovery <------------------------+           |
                                                       v
                                  succeeded / failed / cancelled -> append event
```

The runtime reconstructs task state and execution metadata by replaying an append-only event log. Each state transition and completed-command trace is appended and synchronized before its in-memory update. A running task with a trace for the same attempt is reconciled to that trace's terminal result instead of being executed twice; a running task without a completed trace is requeued with its attempt count preserved.

Retries are explicit, apply only to failed tasks, and stop at a caller-supplied maximum attempt count. The runtime does not infer that an arbitrary command is idempotent. The durable trace stores metadata, not argument values or command output, to reduce accidental secret retention.

The executor accepts only a configured program name allowlist, rejects program paths, canonicalizes the working directory under a fixed workspace, closes stdin, captures bounded output through temporary files, and polls for timeout or cancellation.

The agent loop asks a `ModelProvider` for one structured action at a time and stops at a fixed step count. Tool output is truncated before it becomes the next model observation. The AGY adapter runs as an operator-configured provider boundary in plan/sandbox mode and parses only its structured output. A requested tool does not run through AGY: it returns to the Rust executor and is independently authorized there.

## Security boundary

This is not an OS sandbox. It does not isolate network access, environment variables, filesystem access performed by an allowed command, descendants that escape the direct child, CPU/memory usage, or platform-specific privilege boundaries. Allowed programs and arguments must still be treated as capabilities. A crash after an external side effect but before its execution trace is synced cannot be made exactly-once by this local log; side-effecting tools need idempotency keys or a transactional adapter.
