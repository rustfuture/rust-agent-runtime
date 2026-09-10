# Rust Agent Runtime

A durable task-state baseline for a bounded coding-agent runtime.

The first milestone intentionally contains no LLM and no arbitrary shell execution. It provides append-only task events, idempotent enqueue, attempts, cancellation, and restart recovery that requeues interrupted running tasks.

```bash
cargo test --locked
```

The bounded executor rejects program paths and non-allowlisted programs, canonicalizes the working directory under the configured workspace, kills timed-out children and Unix descendants in their process group, disables stdin, and truncates captured stdout/stderr at a configured byte limit. These controls are not an OS sandbox and do not by themselves isolate network access. If a descendant inherits a pipe and keeps it open after the direct child exits, the executor stops reading after a short bounded grace period instead of blocking.

The provider adapter runs its child process under the same supervisor: a dedicated process group, a local wall-clock timeout, a captured-output byte limit, and cancellation. The runtime's `--print-timeout` is a secondary bound; a cancelled task kills the active provider process group, and streaming is stopped by a watchdog. Killing a timed-out or cancelled process is termination, not isolation.

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

The end-to-end fixture evaluation suite tests the runtime against failing Rust tests across multiple defect families, giving the model only bounded file read/exact replacement and allowlisted `cargo` execution. See [`evaluation/fixture-evaluation-report.md`](evaluation/fixture-evaluation-report.md) for the multi-fixture matrix (covering `off_by_one`, `clamp_range`, and `prefix_format`), patch diffs, latency, token usage, and limitation analysis.

After any edit, the agent loop rejects a model-declared finish until an explicitly configured verification command succeeds. Program and argument tokens must match exactly; an empty verification list permits no command to verify an edit. The fixture example permits `cargo test`, not `cargo --version` or `cargo test --help`. Each new edit invalidates the previous verification. A passing command is still not proof of semantic correctness: project-specific evaluation must independently check the intended behavior, and a no-edit finish is not a demonstrated repair.

The multi-fixture evaluation includes successful repairs (2/4), a budget-limited failure (1/4), and an unresolved premature exit (1/4), giving a 50.0% completion rate on the synthetic suite. This metric is documented truthfully as a bounded suite measurement rather than a generalized autonomy claim.

On macOS, callers can opt into `Isolation::MacOsSandbox`. The generated Seatbelt profile denies network access and denies writes outside the canonical workspace; a platform-gated test executes a real shell and proves both the denied outside write and an allowed inside write. This backend depends on the currently installed `/usr/bin/sandbox-exec`. No equivalent Linux backend is implemented yet, so the same isolation claim is not made there.

The included terminal dashboard exposes durable task state and the latest tool/duration without modifying worker state:

```bash
cargo run --locked -- enqueue ./runtime-data demo-task
cargo run --locked -- status ./runtime-data
cargo run --locked -- watch ./runtime-data 500
```

`cancel` accepts only queued/running tasks and is idempotent for an already-cancelled task. `status` and `watch` use a read-only replay path, so observing a running task cannot trigger restart recovery.

The `run` command connects a durable task to the agent loop:

```bash
AGENT_TASK="make the failing test pass" AGENT_ALLOWED="cargo" AGENT_VERIFY="cargo test" \
  cargo run --locked -- run ./runtime-data demo-task ./fixture
```

It enqueues the id if absent, records `running` before any model call, persists each completed tool run as a tool trace, and writes a terminal state. A separate `cancel DATA_DIR ID` process is observed through the event log and signals the live worker's cancellation token (the CLI `cancel` uses the read-only replay path so it does not requeue a running task as a side effect). Tool traces are stored apart from task-level traces, so an interrupted worker is requeued on restart rather than being mistaken for a completed command. A crash after an external side effect but before its trace is synced cannot be made exactly-once by this local log; side-effecting tools still need idempotency keys.

See `docs/architecture.md` for the trust boundary and `RELEASE_NOTES.md` for the current candidate scope. Licensed under MIT.
