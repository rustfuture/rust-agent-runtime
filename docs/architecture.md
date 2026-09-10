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

The executor accepts only a configured program name allowlist, rejects program paths, canonicalizes the working directory under a fixed workspace, closes stdin, captures bounded output from pipes, and polls for timeout or cancellation. On Unix, it starts each direct child in a new process group and terminates that group on timeout/cancellation; a real shell-descendant test covers this behavior. If a descendant inherits the stdout/stderr pipe and keeps it open after the direct child exits, the reader is not joined indefinitely: the executor waits a short bounded grace period, then returns the output captured so far.

The provider adapter runs its child process under the same supervisor: a dedicated process group, a local wall-clock timeout, a captured-output byte limit, and cancellation. The CLI's own `--print-timeout` is a secondary bound, not the only one. A cancelled task kills the active provider process group; a streaming call is stopped by a watchdog thread that enforces the same timeout.

The agent loop asks a `ModelProvider` for one structured action at a time and stops at a fixed step count. Tool output is truncated before it becomes the next model observation. The AGY adapter runs as an operator-configured provider boundary in plan/sandbox mode and parses only its structured output. A requested tool does not run through AGY: it returns to the Rust executor and is independently authorized there.

For user-visible text, the AGY adapter can consume newline-delimited stream events, emit each text delta immediately through a callback, and return the final token/latency metadata. Structured planning remains a complete schema-constrained response because an incomplete action cannot be safely executed.

An edit creates a verification debt. A subsequent successful bounded command clears that runtime-level debt; an attempted finish before then is rejected and returned as another observation. This prevents an untested edit from being reported as complete, but it does not prove the command was the semantically correct test. Evaluation therefore reruns its declared acceptance test independently.

Workspace reads and edits accept only plain relative paths whose canonical target remains under the configured root. Reads reject oversized or non-regular files. An edit is an exact single-occurrence replacement, has a post-edit size cap, preserves permissions, syncs a temporary file, and atomically renames it over the target. Absolute paths, parent components, symlink escapes, ambiguous matches, empty matches, and new-file creation are rejected.

On macOS, the executor can additionally wrap a command in an opt-in Seatbelt profile through `/usr/bin/sandbox-exec`. That profile denies network operations and filesystem writes outside the canonical workspace. This is verified with a real subprocess test. The default remains `Isolation::None` for portability, and no Linux isolation backend is claimed.

The CLI provides enqueue/cancel commands plus one-shot and refreshing status views. Status uses `Runtime::inspect`, which replays state without performing restart recovery. Only a worker opening the runtime through `Runtime::open` reconciles an interrupted task. Terminal states cannot be cancelled or completed again through the public transition methods.

## Security boundary

Without the opt-in macOS backend, this is not an OS sandbox. Even with it, environment-variable secrecy, CPU/memory usage, and all platform-specific privilege boundaries are not solved. Unix descendant termination is covered, but commands can still create side effects before a timeout. SIGKILL-terminating a timed-out or cancelled child is termination, not isolation: it does not prevent side effects that already happened or restrict what a running process could do before it was killed. Allowed programs and arguments must still be treated as capabilities. A crash after an external side effect but before its execution trace is synced cannot be made exactly-once by this local log; side-effecting tools need idempotency keys or a transactional adapter.
