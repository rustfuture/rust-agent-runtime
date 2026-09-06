# Rust Agent Runtime

A durable task-state baseline for a bounded coding-agent runtime.

The first milestone intentionally contains no LLM and no arbitrary shell execution. It provides append-only task events, idempotent enqueue, attempts, cancellation, and restart recovery that requeues interrupted running tasks.

```bash
cargo test --locked
```

The bounded executor rejects program paths and non-allowlisted programs, canonicalizes the working directory under the configured workspace, kills timed-out children, disables stdin, and truncates captured stdout/stderr at a configured byte limit. These controls are not an OS sandbox and do not yet isolate network access or child-process trees.

`Runtime::run_next` deterministically selects the lowest queued task id, persists `running`, executes through the bounded executor, and records `succeeded` only for a zero exit status without timeout; all other outcomes become `failed`.

A clonable cancellation token is checked while a child runs. Cancellation kills the child, marks the execution as cancelled, and persists the task’s `cancelled` state.

Each completed command also appends a durable execution trace containing its attempt, program name, argument count, exit status, timeout/cancellation flags, bounded-output sizes, truncation flag, and duration. Argument values and captured output are deliberately not written to the event log. On restart, a matching completed trace is reconciled to its terminal state instead of repeating that command. Failed tasks can be explicitly requeued with `Runtime::retry(id, max_attempts)`; retries are never automatic, because callers must decide whether an operation is safe to repeat.

The next layer exposes a model-provider trait, a structured AGY provider, and a step-bounded agent loop. The provider uses `gemini-3.8-flash-low` in the included smoke example, records reported token usage and latency, and requests schema-constrained actions in plan/sandbox mode. Model-requested tools still pass through the executor, so model or repository text cannot add a program to the allowlist.

```bash
smoke_dir=$(mktemp -d)
AGY_BIN=/path/to/agy AGY_WORK_DIR="$smoke_dir" cargo run --locked --example agy_smoke
```

The real smoke call on 2026-09-06 returned the required structured `finish` action in 8.522 seconds. AGY reported 35,067 input and 47 output tokens; the unexpectedly high fixed context overhead is why live model calls are kept out of normal CI and used only at explicit evaluation milestones.

`AgyProvider::stream_text` consumes AGY's NDJSON event stream and emits response deltas through a callback while retaining final usage metadata. A real Rust adapter smoke returned `STREAM_OK` in 6.860 seconds with 34,752 input and 3 output tokens:

```bash
stream_dir=$(mktemp -d)
AGY_BIN=/path/to/agy AGY_WORK_DIR="$stream_dir" cargo run --locked --example agy_stream_smoke
```

The first end-to-end fixture evaluation starts from a failing Rust test, gives the model only bounded file read/exact replacement and allowlisted `cargo` execution, and verifies the resulting patch in a fresh copy. See [`evaluation/agy-off-by-one.md`](evaluation/agy-off-by-one.md) for the patch, test outcome, latency, token usage, and limitations.

After any edit, the agent loop rejects a model-declared finish until a later command succeeds. The report separately exposes changed-file count and whether a post-change command passed. Project-specific evaluation still independently reruns the intended test; a successful unrelated command is not treated as semantic correctness.

The fixture report includes both the successful run and a deliberately under-budget `MAX_STEPS=1` failure. That failure is counted in the tiny smoke-set denominator (1 completed / 2 total, 50%), while explicitly not being generalized into an overall agent success rate.

On macOS, callers can opt into `Isolation::MacOsSandbox`. The generated Seatbelt profile denies network access and denies writes outside the canonical workspace; a platform-gated test executes a real shell and proves both the denied outside write and an allowed inside write. This backend depends on the currently installed `/usr/bin/sandbox-exec`. No equivalent Linux backend is implemented yet, so the same isolation claim is not made there.

The included terminal dashboard exposes durable task state and the latest tool/duration without modifying worker state:

```bash
cargo run --locked -- enqueue ./runtime-data demo-task
cargo run --locked -- status ./runtime-data
cargo run --locked -- watch ./runtime-data 500
```

`cancel` accepts only queued/running tasks and is idempotent for an already-cancelled task. `status` and `watch` use a read-only replay path, so observing a running task cannot trigger restart recovery.

See `docs/architecture.md` for the trust boundary and `RELEASE_NOTES.md` for the current candidate scope. Licensed under MIT.
