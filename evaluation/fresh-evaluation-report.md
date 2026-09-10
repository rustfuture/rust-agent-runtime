# Fresh-Fixture Agent Evaluation (independent acceptance)

Date: 2026-09-10
Model: `gemini-3.8-flash-low` via the AGY structured-output provider
Provider binary: `agy 1.1.27`
Runtime: `rust-agent-runtime` (durable worker + bounded executor + macOS seatbelt)
Host: macOS Apple Silicon, `rustc 1.94.1`
Code under test: the commit that adds this report

## What this measures

The durable worker (`rust-agent-runtime run DATA_DIR ID WORKSPACE`) is run on three fresh,
unpublished defect families. The task text gives only the expected behavior and the failing context;
it never contains the expected/replacement strings or the solution code. Success is decided by an
**independent acceptance test** that the model cannot reach: after the run, only the agent's `src/` is
copied over a fresh pristine crate whose canonical test is kept outside the agent workspace, and that
crate is compiled and tested. A model that edited or deleted the visible test cannot pass this gate.

Each family is allowed at most one controlled retry. All attempts are kept.

## Harness

- `evaluation/fresh_fixtures/<family>/` — pristine buggy crate with `tests/acceptance.rs`.
- `evaluation/run_fresh_evaluation.sh` — orchestrates baseline, run, and independent acceptance.
- Runs: `evaluation/runs/<UTC run id>/` with per-attempt worker log, acceptance log, patch, and the
  durable event log under `data/`.

The agent workspace is a separate temporary copy. On macOS the tool executor is wrapped in the opt-in
Seatbelt profile (denies network and writes outside the workspace) and AGY runs with `--sandbox`.
Provider and executor run under the local supervisor with a wall-clock timeout, an output byte cap,
and a process group (see `docs/architecture.md`).

## Results

### Run `20260910T210723Z` (final)

| Family | Worker outcome | Worker terminal state | Independent acceptance | Patch |
|---|---|---|---|---|
| `off_by_one` | `failed` (`TimedOut`, step limit) | failed | **pass** | `for i in 1..n` → `1..=n` |
| `clamp_range` | `completed` | **succeeded** | **pass** | swapped branches to `min`/`max` for out-of-range values |
| `prefix_format` | `failed` (`TimedOut`, step limit) | failed | **pass** | `format!("{}", name.trim())` → `format!("{}{}", prefix, name.trim())` |

Aggregate: **3 / 3 independent acceptance passes**; **1 / 3 clean worker successes**. The two
non-clean cases applied a correct patch but reached the step limit before issuing an accepted
`finish`, because they ran the verification command with extra arguments that the runtime's exact
verification matcher does not accept. This is reported as-is: a correct patch is not the same as a
clean terminal state.

### Run `20260910T205810Z`

Same fixtures before the worker was hardened. Independent acceptance **3 / 3**; every worker run
ended in a step-limit timeout. The patches were already correct.

### Run `20260910T204955Z` (defect discovery)

The first run exposed a real product defect: the model returned `finish` describing a change it had
not applied (`changed_files=0`, `tool_runs=0`) and the worker recorded `succeeded`. This motivated two
fixes included in the code under test:

1. `AgentLoop::with_required_edit(true)` (used by the `run` CLI) rejects a `finish` until at least one
   edit has been applied.
2. `worker::run_agent_task` treats a finish with no change, or with an unverified change, as a failure
   rather than a success.

The harness in this run also had a missing `mkdir` that prevented the acceptance copy; that is fixed
in the script and does not affect later runs.

## Honest limitations

- Three tiny synthetic crates are not a general benchmark and give no autonomy claim.
- The model sometimes applies the correct fix but does not reach a clean terminal state within the
  step budget. Both the timeout and the correct patch are reported; neither is hidden.
- Verification matching is intentionally exact (`cargo test`, not `cargo test --quiet`). The system
  prompt now lists the exact `verification_programs`, but the model does not always comply.
- Token/cost figures are provider-reported and may omit fixed context overhead; this run is not a cost
  study.
- The `run` CLI is exercised with the real provider. Deterministic worker behavior (durable state,
  separate tool traces, cancellation, restart recovery) is covered by `cargo test`, not by these runs.
