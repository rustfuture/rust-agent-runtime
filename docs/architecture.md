# Architecture

```text
enqueue -> append event -> queued -> running -> bounded executor
   ^                                      |           |
   |                                      |           +-> timeout / cancel / output cap
restart recovery <------------------------+           |
                                                       v
                                  succeeded / failed / cancelled -> append event
```

The runtime reconstructs task state by replaying an append-only event log. Each state transition is appended and synchronized before in-memory state changes. A running task found during restart is requeued with its attempt count preserved.

The executor accepts only a configured program name allowlist, rejects program paths, canonicalizes the working directory under a fixed workspace, closes stdin, captures bounded output through temporary files, and polls for timeout or cancellation.

## Security boundary

This is not an OS sandbox. It does not isolate network access, environment variables, filesystem access performed by an allowed command, descendants that escape the direct child, CPU/memory usage, or platform-specific privilege boundaries. Allowed programs and arguments must still be treated as capabilities.
