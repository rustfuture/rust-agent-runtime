# Fresh-Fixture Agent Evaluation (independent acceptance)

Date: 2026-09-10/11
Model: `gemini-3.8-flash-low` via the AGY structured-output provider
Provider binary: `agy 1.2.0`
Runtime: `rust-agent-runtime` (durable worker + bounded executor + macOS seatbelt)
Host: macOS Apple Silicon, `rustc 1.94.1`
Code under test: commits `23925b1` (explicit verify action) and `6683999` (trustworthy harness +
controls); real runs additionally record the harness and binary hashes below.
Historical runs from before these commits are preserved under `evaluation/runs/`.

## What this measures

The durable worker (`rust-agent-runtime run DATA_DIR ID WORKSPACE`) is run on three fresh,
unpublished defect families. The task text gives only the expected behavior and the failing context;
it never contains the expected/replacement strings or the solution code. Success is decided by an
**independent acceptance test** that the model cannot reach: after the run, only the agent's `src/` is
copied over a fresh pristine crate whose canonical test is kept outside the agent workspace, and that
crate is compiled and tested. A model that edited or deleted the visible test cannot pass this gate.

Each family is allowed at most one controlled retry. All attempts are kept, and a family passes only
when one single attempt satisfies every gate; acceptance from one attempt is never combined with a
clean worker from another.

## Verification contract (what changed for this round)

The previous matcher accepted `run_tool` as verification only when the program and argv matched the
configured command **exactly**. Real traces show why that failed: successful cargo invocations with
extra arguments (`argc=2`/`argc=3`, `status=0`) were never accepted, so a correct patch could reach the
step limit without a clean terminal state
(`evaluation/runs/20260910T204955Z/data/*/events.log`, `.../20260910T205810Z/data/*/events.log`).

The runtime now exposes an explicit `verify` action. The model cannot add flags, chain commands, or
substitute a program: the runtime itself runs the operator-configured `AGENT_VERIFY` command verbatim
through the bounded executor, and only a passing result clears the edit debt. A successful `run_tool`
never verifies. Regression tests cover: finish before any edit rejected, edit → verify → finish
accepted, verify → new edit → finish rejected until re-verified, run_tool with extra flags not
accepted, unconfigured verification denied, timeout during verify keeps the debt, and the existing
output-cap, cancellation, and restart-recovery behavior.

## Harness trust properties

- `cargo build --locked` runs first; on failure the run records `build_failure`, exits 2, and never
  uses a stale binary. The built binary sha256, source sha, harness sha, dirty count, provider
  version, limits, and exact commands are recorded in `metadata.txt`.
- A baseline crate that fails to build is recorded as a harness/configuration error, never as an
  expected baseline failure. An unexpected baseline pass is recorded as a configuration error too.
- Fixture/acceptance copy errors are fatal signals (`harness_ok=false`); before acceptance the copy is
  compared (`cmp`) against the agent's `src/`, so acceptance can never silently run on pristine code.
- A colliding `RUN_ID` is suffixed (`-1`, `-2`, …); existing evidence is preserved.
- One family failing cannot stop the other families.
- Process exit code: `0` only when every family passes baseline, harness, independent acceptance, and
  a clean worker terminal state **within the same attempt**; `2` for any harness/build failure; `1`
  for partial results. Per-attempt rows stay in `<family>-attempts.tsv`, and the per-family record
  always describes a single attempt (the passing one if any, otherwise the latest tried), so
  acceptance from one attempt can never be combined with a clean worker from another.
- `summary.tsv` records per-family `baseline_failed_as_expected`, `harness_ok`,
  `patch_acceptance_pass`, `worker_clean_success`, `failure_kind`
  (`step_limit | wall_timeout | cancelled | provider_error | other | none`), attempts, duration, and
  tokens; `<family>.result` additionally records `family_pass`, `attempts_acceptance_pass`, and
  `attempts_worker_clean` as separate counters that never feed the pass gate.
- Only the temporary work directory created by the run is removed; all evidence stays.

Fixtures set `doctest = false`: rustdoc creates its scratch directory in `$TMPDIR`, which the macOS
Seatbelt profile (writes restricted to the workspace) correctly denies. These fixtures have no
doctests, so this changes nothing about the acceptance semantics.

## Deterministic controls (no real model)

`evaluation/run_controls.sh` drives the real harness through `AGENT_FAKE_PROVIDER`: a local script
emits AGY-shaped JSON envelopes through the same supervised provider path, so the local wall-clock
timeout, captured-output cap, process group, and cancellation remain enforced, and tool programs still
pass through the executor allowlist (also covered by `src/provider.rs` unit tests and
`tests/cli_fake_provider.rs`).

Final control run `evaluation/runs/controls-20260911T203623Z-*` (36 checks, 0 failures):

| Control | Expected | Observed |
|---|---|---|
| build/configuration failure (`EVAL_CARGO_BIN` fails) | exit 2, `build_failure` marker, no family run | pass |
| invalid baseline (baseline unexpectedly passes) | exit 2, `baseline_failed_as_expected=false` | pass |
| acceptance failure (agent weakens the visible test) | exit 1, worker clean, acceptance fails | pass |
| correct patch but no verify (step limit) | exit 1, acceptance pass, `failure_kind=step_limit` | pass |
| correct patch but provider wall timeout | exit 1, acceptance pass, `failure_kind=wall_timeout` | pass |
| opposite attempts (attempt 1 acceptance pass + worker failure, attempt 2 clean worker + acceptance failure) | exit 1, `family_pass=false`, both attempts recorded separately | pass |
| fully successful run | exit 0, acceptance pass, worker clean, `failure_kind=none` | pass |
| mixed families (missing fixture then off_by_one) | exit 2, failing family recorded, later family still runs and passes | pass |
| same `RUN_ID` rerun | first evidence preserved, second run suffixed `-1` | pass |

### Attempt-consistency regression (2026-09-11)

The base harness (commit `a6cb005`) latched `family_acceptance` and `family_clean` independently
across attempts (`evaluation/run_fresh_evaluation.sh:340-345` and `:362` on that base). An attempt
that passed acceptance but failed the worker, followed by an attempt that was worker-clean but failed
acceptance, combined into `patch_acceptance_pass=true` + `worker_clean_success=true` and exit 0 even
though no single attempt met both gates. The fix records each attempt as its own row and computes the
family pass only from a single attempt (baseline failed as expected, harness ok, independent
acceptance pass, clean worker, `failure_kind=none`); the separate `attempts_acceptance_pass` and
`attempts_worker_clean` counters are evidence only and never feed the pass gate.

Reproduced against the base harness with the new
`evaluation/controls/fake_providers/opposite_attempts.sh`: attempt 1 applies the correct
`1..n` → `1..=n` patch and reaches the step limit; attempt 2 cleanly finishes after weakening the
visible test. Base result: `patch_acceptance_pass=true`, `worker_clean_success=true`,
`failure_kind=none`, `exit_status=0`. Fixed result: `family_pass=false`, `exit_status=1`, with
attempt 1 recorded as `acceptance=true, worker_clean=false, failure_kind=step_limit` and attempt 2 as
`acceptance=false, worker_clean=true, failure_kind=none` in
`evaluation/runs/controls-20260911T203623Z-opposite-attempts/`.

## Real-model run `20260910T215500Z-real` (primary)

Command:

```bash
RUN_ID=20260910T215500Z-real AGY_BIN=agy AGY_MODEL=gemini-3.8-flash-low \
  AGENT_MAX_STEPS=6 AGENT_TIMEOUT_SECS=120 bash evaluation/run_fresh_evaluation.sh
```

Metadata: `source_sha=6683999ac5d150591f726a9c68dfecfd1f8a12f1`,
`harness_sha256=801cd3da31904f87bb0b03fa9324bab67902712c7016e925ff6c745c5a6513ed`,
binary sha256 `974966a8280f11a52a52074554526291fb4a2ddd03a62ac1c47af0b120942683`,
`source_dirty_count=2` (the preserved untracked `tests/fixtures/unwrap-panic/Cargo.lock` plus this
run's own evidence directory; the later harness revision measures dirty state before creating its
run directory).

| Family | Baseline | Independent acceptance | Worker terminal state | failure_kind | Attempts | Duration | Input tokens | Output tokens | Patch |
|---|---|---|---|---|---|---|---|---|---|
| `off_by_one` | failed as expected | **pass** | **succeeded** | none | 1 | 94 s | 182,712 | 10,487 | `1..n` → `1..=n` |
| `clamp_range` | failed as expected | **pass** | **succeeded** | none | 1 | 33 s | 145,338 | 466 | swapped `min`/`max` branches |
| `prefix_format` | failed as expected | **pass** | **succeeded** | none | 1 | 97 s | 230,102 | 16,459 | `format!("{}", name.trim())` → `format!("{}{}", prefix, name.trim())` |

Aggregate: **3/3 independent acceptance passes**, **3/3 clean worker successes**, no retries needed,
no step-limit or wall-timeout failures. Every event log ends with `succeeded` and each shows the
runtime-run verification (`tool cargo argc=1`) with status 0.

## Real-model confirmation run `20260910T215600Z-real2`

A second run confirmed the result after a metadata-only harness change (dirty count measured before
the run directory is created). Metadata: `source_sha=6683999...`, `harness_sha256=6033c654...`,
`source_dirty_count=3` (untracked fixture lock, the uncommitted metadata fix, and the first run
directory).

| Family | Baseline | Acceptance | Worker terminal state | failure_kind | Attempts | Duration | Notes |
|---|---|---|---|---|---|---|---|
| `off_by_one` | failed as expected | pass (attempt 2) | failed (both attempts) | provider_error | 2 | 70 s | AGY returned `503 The service is currently unavailable` on a provider call in both attempts |
| `clamp_range` | failed as expected | pass | succeeded | none | 1 | 71 s | clean |
| `prefix_format` | failed as expected | pass | succeeded | none | 1 | 53 s | clean |

Attempt 1 of `off_by_one` was cut short by the 503 before the patch landed; attempt 2 applied the
correct patch and the runtime verification passed, but the next provider call also received 503, so
the worker could not finish. This is an external provider-availability failure, not a step-limit or
wall-timeout failure, and it is reported as-is.

Aggregate across both real runs: **6/6 independent acceptance passes**, **5/6 clean worker
successes**; the single non-clean case is the provider 503 above. No fixed claim beyond this suite is
made: three tiny synthetic crates are not a general benchmark.

## Historical evidence (preserved)

- `evaluation/runs/20260910T210723Z` — pre-fix round: acceptance 3/3, clean worker 1/3; the two
  non-clean cases ran verification with extra arguments that exact argv matching rejected.
- `evaluation/runs/20260910T205810Z` — pre-hardening: acceptance 3/3, all workers step-limited.
- `evaluation/runs/20260910T204955Z` — defect discovery: premature no-edit finish and the missing
  `mkdir` acceptance bug.
- `evaluation/runs/controls-20260910T214922Z-*` and `controls-20260910T215842Z-*` — deterministic
  control runs (24/24 checks each).

## Commands run for this round

```bash
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
bash evaluation/run_controls.sh
RUN_ID=20260910T215500Z-real  ... bash evaluation/run_fresh_evaluation.sh
RUN_ID=20260910T215600Z-real2 ... bash evaluation/run_fresh_evaluation.sh
```

## Honest limitations

- Three tiny synthetic crates are not a general benchmark and give no autonomy claim.
- The confirmation run hit a transient provider 503; provider availability is outside the runtime's
  control, and the run is reported including the failed family and both attempts.
- Token/cost figures are provider-reported and may omit fixed context overhead; this is not a cost
  study.
- Step-limit and wall-timeout behavior is covered by deterministic controls and unit tests, not by the
  real-model runs (which produced neither in this round).
- Acceptance tests exercise intended behavior, not full semantic correctness; the independent copy
  only takes `src/`, so test tampering cannot pass.
