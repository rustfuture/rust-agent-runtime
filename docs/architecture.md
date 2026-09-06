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

Workspace reads and edits accept only plain relative paths whose canonical target remains under the configured root. Reads reject oversized or non-regular files. An edit is an exact single-occurrence replacement, has a post-edit size cap, preserves permissions, syncs a temporary file, and atomically renames it over the target. Absolute paths, parent components, symlink escapes, ambiguous matches, empty matches, and new-file creation are rejected.

On macOS, the executor can additionally wrap a command in an opt-in Seatbelt profile through `/usr/bin/sandbox-exec`. That profile denies network operations and filesystem writes outside the canonical workspace. This is verified with a real subprocess test. The default remains `Isolation::None` for portability, and no Linux isolation backend is claimed.

The CLI provides enqueue/cancel commands plus one-shot and refreshing status views. Status uses `Runtime::inspect`, which replays state without performing restart recovery. Only a worker opening the runtime through `Runtime::open` reconciles an interrupted task. Terminal states cannot be cancelled or completed again through the public transition methods.

## Security boundary

Without the opt-in macOS backend, this is not an OS sandbox. Even with it, environment-variable secrecy, descendant lifecycle containment, CPU/memory usage, and all platform-specific privilege boundaries are not solved. Allowed programs and arguments must still be treated as capabilities. A crash after an external side effect but before its execution trace is synced cannot be made exactly-once by this local log; side-effecting tools need idempotency keys or a transactional adapter.
